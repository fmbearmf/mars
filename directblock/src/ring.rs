//! formally verified lock-free SPSC ring buffer channel.
//!
//! ## Scope/Design
//!
//! Implicit drop does not perform channel cleanup; closure must be explicitly called.
//! The contract of `Arc::try_unwrap` is trusted and unverified.
//!
//! ## The Abstract Model
//!
//! Our requirement is that this is an ideal FIFO queue:
//! - Values are removed in the order they were pushed.
//! - After consumer closure is detected, pushes return `Disconnected`.
//!   After producer closure is detected and the queued values are drained, pops return `Disconnected`.
//! - There are no data races, out-of-bounds accesses, reads of uninitialized memory, or double frees.
//! - When an endpoint closes, peer read/writes fail gracefully with typed errors
//!
//! ## Informal Proof
//!
//! The verification relies on 5 main components:
//!
//! 1. The State Machine
//!
//! We model our monotonic indices as infinite natural numbers `head` and `tail`.
//! - `head` counts successful pushes.
//! - `tail` counts removals, including values discarded during cleanup.
//!
//! We enforce this invariant:
//! ```
//! tail <= head <= tail + CAPACITY
//! ```
//!
//! The active (logical) window is `[tail, tail + CAP)`
//! For slots not currently checked out by an endpoint:
//! - `[tail, head)` contains initialized values matching `sent[tail..head]`.
//! - `[head, tail + CAP)` contains uninitiailized slots.
//!
//! Distinct logical indices in this window each map to distinct physical slots modulo `CAP`,
//! because the window's length is `CAP`.
//!
//! The consumer checks out an initialized slot at `tail`.
//! The producer checks out an uninitialized slot at `head` only if there is capacity remaining.
//! These slots cannot alias.
//! While a slot is checked out, its endpoint exclusively owns its permission; the `busy` state accounts for changes before commit.
//!
//! 2. Exclusive Ownership + Borrows
//!
//! Access to a buffer cell requires its exclusive, non-duplicable permission token.
//! Tokens can be transferred or discarded, but never duplicated.
//!
//! The consumer checks out the slot `tail`, extracts its value,
//! and returns its permission as uninitialized under the logical index `tail + CAP`,
//! while advancing `tail`.
//!
//! Neither thread can access a buffer cell without holding its token.
//!
//! 3. Hardware
//!
//! Machine counters are projections of the logical counters modulo `2^W`,
//! where `W` is the width of usize.
//! We require `2 <= CAP <= 2^W / 2` and `CAP` to divide `2^W`.
//! Since `0 <= head - tail <= CAP < 2^W`, wrapping the subtraction of projected counters
//! equals the logical distance.
//! Since `CAP` divides `2^W`, projecting some logical index before taking it modulo `CAP` selects the same physical slot.
//!
//! 4. Caches of Atomics
//!
//! The producer caches `tail` in `shadow_tail`; the the consumer caches `head` in `shadow_head`.
//!
//! Each cache is a lower bound on its matching logical counter.
//! A stale tail therefore only underestimates free capacity, and
//! a stale head underestimates the available data.
//!
//! The purpose of cached capacity and data is avoiding refreshing the peer's index.
//! Successful operations still publish their own index atomically, and pushes check the consumer's closure flag.
//!
//! 5. Lifecycle
//!
//! The atomic flags `producer_alive` and `consumer_alive` record explicit endpoint closure,
//! as opposed to `Drop`.
//! Producer closure prevents any furhter pushes;
//! the consumer can still drain queued values before reporting disconnection.
//!
//! Consumer closure prevents further pops through that endpoint and drains up to `CAP` values.
//! A push already in progress can still complete.
//!
//! On success, `close_channel` recovers exclusive ownership of the shared ring,
//! and drops remaining initialized values using their permissions.
//!
//! Closure is optional. Discarding the endpoints or closed handles can leave queued values undropped.
//!

// RingProtocol
#![allow(non_snake_case)]

use alloc::{sync::Arc, vec::Vec};
use core::{fmt, marker::PhantomData};
#[cfg(verus_keep_ghost)]
// greatest thing since sliced bread. oh my god bruh.
use verus_state_machines_macros::tokenized_state_machine;
use vstd::arithmetic::div_mod::*;
use vstd::atomic_ghost::*;
use vstd::cell::CellId;
use vstd::cell::pcell_maybe_uninit::{self as cell, PCell};
use vstd::invariant::{AtomicInvariant, InvariantPredicate};
use vstd::open_atomic_invariant;
use vstd::prelude::*;

verus! {
#[derive(Debug, PartialEq, Eq)]
pub enum PushError<T> { Full(T), Disconnected(T) }
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PopError { Empty, Disconnected }
}
impl<T> fmt::Display for PushError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(_) => write!(f, "queue is full"),
            Self::Disconnected(_) => write!(f, "consumer has disconnected"),
        }
    }
}
impl<T: fmt::Debug> core::error::Error for PushError<T> {}
impl fmt::Display for PopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "queue is empty"),
            Self::Disconnected => write!(f, "producer disconnected"),
        }
    }
}
impl core::error::Error for PopError {}

verus! {
impl<T> PushError<T> {
    pub fn into_inner(self) -> T {
        match self { Self::Full(v) | Self::Disconnected(v) => v }
    }
}

pub open spec fn word() -> int { usize::MAX as int + 1 }
pub open spec fn project(n: nat) -> int { n as int % word() }
pub open spec fn valid_capacity(cap: nat) -> bool {
    2 <= cap && cap <= word() / 2 && word() % (cap as int) == 0
}

pub struct WriteState<T> {
    pub pos: nat,
    pub shadow: nat,
    pub busy: bool,
    pub closed: bool,
    pub consumer_closed: bool,
    pub sent: Seq<T>,
}
pub struct ReadState<T> {
    pub pos: nat,
    pub shadow: nat,
    pub busy: bool,
    pub pending: Option<T>,
    pub closing: bool,
    pub closed_head: Option<nat>,
    pub received: Seq<T>,
}

proof fn slot_wrap(n: nat, cap: nat)
    requires cap > 0
    ensures (n + cap) % cap == n % cap
{
    lemma_mod_adds(n as int, cap as int, cap as int);
}

#[cfg(verus_keep_ghost)]
tokenized_state_machine! {
    RingProtocol<T> {
    fields {
        #[sharding(constant)] pub cells: Seq<CellId>,
        #[sharding(variable)] pub storage: Map<nat, cell::PointsTo<T>>,
        #[sharding(variable)] pub head: nat,
        #[sharding(variable)] pub tail: nat,
        #[sharding(variable)] pub writer: WriteState<T>,
        #[sharding(variable)] pub reader: ReadState<T>,
        #[sharding(variable)] pub producer_alive: bool,
        #[sharding(variable)] pub consumer_alive: bool,
    }
    pub open spec fn cap(&self) -> nat { self.cells.len() }
    pub open spec fn checked_out(&self, n: nat) -> bool {
        (self.writer.busy && n == self.head) || (self.reader.busy && n == self.tail)
    }
    pub open spec fn valid_slot(&self, n: nat) -> bool {
        if self.checked_out(n) {
            !self.storage.dom().contains(n)
        } else {
            self.storage.dom().contains(n)
            && self.storage[n].id() == self.cells[(n % self.cap()) as int]
            && if n < self.head {
                self.storage[n].is_init() && self.storage[n].value() == self.writer.sent[n as int]
            } else {
                self.storage[n].is_uninit()
            }
        }
    }
    #[invariant]
    pub fn bounds(&self) -> bool {
        &&& self.cap() >= 2
        &&& self.tail <= self.head <= self.tail + self.cap()
        &&& self.writer.pos == self.head
        &&& self.reader.pos == self.tail
        &&& self.writer.shadow <= self.tail
        &&& self.head <= self.writer.shadow + self.cap()
        &&& self.tail <= self.reader.shadow <= self.head
        &&& (self.writer.busy ==> self.head < self.writer.shadow + self.cap())
        &&& (self.reader.busy ==> self.tail < self.reader.shadow)
        &&& (self.writer.closed ==> !self.writer.busy)
        &&& self.producer_alive == !self.writer.closed
        &&& self.consumer_alive == !self.reader.closing
        &&& (self.writer.consumer_closed ==> !self.consumer_alive)
        &&& (self.reader.closed_head.is_some() ==> !self.producer_alive
            && self.reader.closed_head == Some(self.head))
    }
    #[invariant]
    pub fn fifo(&self) -> bool {
        &&& self.writer.sent.len() == self.head
        &&& self.reader.received.len() == self.tail
        &&& self.reader.received == self.writer.sent.take(self.tail as int)
        &&& (self.reader.busy ==> self.reader.pending == Some(self.writer.sent[self.tail as int]))
    }
    #[invariant]
    pub fn storage_wf(&self) -> bool {
        &&& forall|n: nat| self.tail <= n < self.tail + self.cap() ==> self.valid_slot(n)
        &&& forall|n: nat| #[trigger] self.storage.dom().contains(n) ==> self.tail <= n < self.tail + self.cap()
    }
    init! {
        initialize(cells: Seq<CellId>, storage: Map<nat, cell::PointsTo<T>>) {
            require(cells.len() >= 2);
            require(forall|n: nat| 0 <= n < cells.len() ==>
                #[trigger] storage.dom().contains(n)
                && storage[n].id() == cells[n as int] && storage[n].is_uninit());
            require(forall|n: nat| #[trigger] storage.dom().contains(n) ==> n < cells.len());
            init cells = cells;
            init storage = storage;
            init head = 0;
            init tail = 0;
            init writer = WriteState { pos: 0, shadow: 0, busy: false, closed: false,
                consumer_closed: false, sent: Seq::empty() };
            init reader = ReadState { pos: 0, shadow: 0, busy: false, pending: None,
                closing: false, closed_head: None, received: Seq::empty() };
            init producer_alive = true;
            init consumer_alive = true;
        }
    }
    transition! {
        refresh_tail() {
            require(!pre.writer.busy && !pre.writer.closed);
            assert(pre.tail <= pre.writer.pos <= pre.tail + pre.cells.len());
            update writer = WriteState { shadow: pre.tail, ..pre.writer };
        }
    }
    transition! {
        refresh_head() {
            require(!pre.reader.busy);
            assert(pre.reader.pos <= pre.head <= pre.reader.pos + pre.cells.len());
            assert(pre.reader.closed_head.is_some() ==> pre.reader.closed_head == Some(pre.head));
            update reader = ReadState { shadow: pre.head, ..pre.reader };
        }
    }
    transition! {
        produce_start(perm: cell::PointsTo<T>) {
            require(!pre.writer.busy && !pre.writer.closed);
            require(pre.writer.pos < pre.writer.shadow + pre.cells.len());
            let n = pre.writer.pos;
            require(pre.storage.dom().contains(n) && perm == pre.storage[n]);
            update storage = pre.storage.remove(n);
            assert(perm.id() == pre.cells[(n % pre.cells.len()) as int] && perm.is_uninit()) by {
                assert(pre.valid_slot(n));
            };
            update writer = WriteState { busy: true, ..pre.writer };
        }
    }
    transition! {
        produce_end(item: T, perm: cell::PointsTo<T>) {
            require(pre.writer.busy);
            let n = pre.writer.pos;
            require(perm.id() == pre.cells[(n % pre.cells.len()) as int]);
            require(perm.is_init() && perm.value() == item);
            assert(!pre.storage.dom().contains(n)) by { assert(pre.valid_slot(n)); };
            update storage = pre.storage.insert(n, perm);
            update head = n + 1;
            update writer = WriteState { pos: n + 1, busy: false, sent: pre.writer.sent.push(item), ..pre.writer };
        }
    }
    transition! {
        consume_start(perm: cell::PointsTo<T>) {
            require(!pre.reader.busy);
            require(pre.reader.pos < pre.reader.shadow);
            let n = pre.reader.pos;
            require(pre.storage.dom().contains(n) && perm == pre.storage[n]);
            update storage = pre.storage.remove(n);
            assert(perm.id() == pre.cells[(n % pre.cells.len()) as int] && perm.is_init()) by {
                assert(pre.valid_slot(n));
            };
            let value = perm.value();
            update reader = ReadState { busy: true, pending: Some(value), ..pre.reader };
        }
    }
    transition! {
        consume_end(perm: cell::PointsTo<T>) {
            require(pre.reader.busy);
            let n = pre.reader.pos;
            require(perm.id() == pre.cells[(n % pre.cells.len()) as int] && perm.is_uninit());
            assert(!pre.storage.dom().contains(n + pre.cells.len()));
            update storage = pre.storage.insert(n + pre.cells.len(), perm);
            update tail = n + 1;
            update reader = ReadState { pos: n + 1, busy: false, pending: None,
                received: pre.reader.received.push(pre.reader.pending.unwrap()), ..pre.reader };
        }
    }
    transition! {
        close_producer() {
            require(!pre.writer.busy && !pre.writer.closed);
            update writer = WriteState { closed: true, ..pre.writer };
            update producer_alive = false;
        }
    }
    transition! {
        close_consumer() {
            require(!pre.reader.busy && !pre.reader.closing);
            update reader = ReadState { closing: true, ..pre.reader };
            update consumer_alive = false;
        }
    }
    transition! {
        note_consumer_closed() {
            require(!pre.consumer_alive);
            update writer = WriteState { consumer_closed: true, ..pre.writer };
        }
    }
    transition! {
        note_producer_closed() {
            require(!pre.producer_alive);
            birds_eye let final_head = pre.head;
            update reader = ReadState { closed_head: Some(final_head), ..pre.reader };
        }
    }
    property! {
        cleanup_ready() {
            require(pre.writer.closed && pre.reader.closing);
            require(!pre.writer.busy && !pre.reader.busy);
            assert(pre.tail <= pre.head <= pre.tail + pre.cells.len());
            assert(forall|n: nat| pre.tail <= n < pre.head ==>
                #[trigger] pre.storage.dom().contains(n)
                && pre.storage[n].id() == pre.cells[(n % pre.cells.len()) as int]
                && pre.storage[n].is_init()) by {
                assert forall|n: nat| pre.tail <= n < pre.head implies
                    #[trigger] pre.storage.dom().contains(n)
                    && pre.storage[n].id() == pre.cells[(n % pre.cells.len()) as int]
                    && pre.storage[n].is_init() by {
                    assert(pre.valid_slot(n));
                }
            };
        }
    }
    property! {
        produce_ready() {
            require(!pre.writer.busy && !pre.writer.closed);
            require(pre.writer.pos < pre.writer.shadow + pre.cells.len());
            let n = pre.writer.pos;
            assert(pre.storage.dom().contains(n)
                && pre.storage[n].id() == pre.cells[(n % pre.cells.len()) as int]
                && pre.storage[n].is_uninit()) by { assert(pre.valid_slot(n)); };
        }
    }
    property! {
        consume_ready() {
            require(!pre.reader.busy && pre.reader.pos < pre.reader.shadow);
            let n = pre.reader.pos;
            assert(pre.storage.dom().contains(n)
                && pre.storage[n].id() == pre.cells[(n % pre.cells.len()) as int]
                && pre.storage[n].is_init()) by { assert(pre.valid_slot(n)); };
        }
    }
    property! {
        inspect_fifo() {
            assert(pre.reader.received == pre.writer.sent.take(pre.reader.received.len() as int));
        }
    }
    property! {
        inspect_writer_tail() { assert(pre.tail <= pre.writer.pos <= pre.tail + pre.cells.len()); }
    }
    property! {
        inspect_reader_head() { assert(pre.reader.pos <= pre.head <= pre.reader.pos + pre.cells.len()); }
    }
    #[inductive(initialize)]
    fn initialize_inductive(post: Self, cells: Seq<CellId>, storage: Map<nat, cell::PointsTo<T>>) {
        assert(post.reader.received =~= post.writer.sent.take(0));
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            lemma_small_mod(n, post.cap());
            assert(storage.dom().contains(n));
            assert(storage[n].id() == cells[n as int]);
        }
    }
    #[inductive(refresh_tail)]
    fn refresh_tail_inductive(pre: Self, post: Self) {
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(refresh_head)]
    fn refresh_head_inductive(pre: Self, post: Self) {
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(produce_start)]
    fn produce_start_inductive(pre: Self, post: Self, perm: cell::PointsTo<T>) {
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(produce_end)]
    fn produce_end_inductive(pre: Self, post: Self, item: T, perm: cell::PointsTo<T>) {
        assert(post.writer.sent.take(post.tail as int) == pre.writer.sent.take(pre.tail as int));
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(consume_start)]
    fn consume_start_inductive(pre: Self, post: Self, perm: cell::PointsTo<T>) {
        assert(pre.valid_slot(pre.tail));
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(consume_end)]
    fn consume_end_inductive(pre: Self, post: Self, perm: cell::PointsTo<T>) {
        slot_wrap(pre.tail, pre.cap());
        assert(pre.valid_slot(pre.tail));
        assert(post.reader.received == post.writer.sent.take(post.tail as int));
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            if n < pre.tail + pre.cap() { assert(pre.valid_slot(n)); }
        }
        assert forall|n: nat| #[trigger] post.storage.dom().contains(n) implies post.tail <= n < post.tail + post.cap() by {
            if n != pre.tail + pre.cap() { assert(pre.storage.dom().contains(n)); }
        }
    }
    #[inductive(close_consumer)]
    fn close_consumer_inductive(pre: Self, post: Self) {
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(note_consumer_closed)]
    fn note_consumer_closed_inductive(pre: Self, post: Self) {
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(note_producer_closed)]
    fn note_producer_closed_inductive(pre: Self, post: Self) {
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
    #[inductive(close_producer)]
    fn close_producer_inductive(pre: Self, post: Self) {
        assert forall|n: nat| post.tail <= n < post.tail + post.cap() implies post.valid_slot(n) by {
            assert(pre.valid_slot(n));
        }
    }
}}

proof fn projection_step(n: nat)
    ensures
        if project(n) == usize::MAX as int { project(n + 1) == 0 }
        else { project(n + 1) == project(n) + 1 }
{
    lemma_mod_bound(n as int, word());
    lemma_mod_adds(n as int, 1, word());
    if project(n) == usize::MAX as int {
        assert((project(n) + 1) / word() == 1);
    } else {
        assert((project(n) + 1) / word() == 0);
    }
}

proof fn projection_distance(a: nat, b: nat)
    requires b <= a, a - b < word()
    ensures
        if project(a) >= project(b) { a - b == project(a) - project(b) }
        else { a - b == word() + project(a) - project(b) }
{
    lemma_mod_bound(a as int, word());
    lemma_mod_bound(b as int, word());
    lemma_small_mod((a - b) as nat, word() as nat);
    lemma_sub_mod_noop(a as int, b as int, word());
    if project(a) >= project(b) {
        lemma_small_mod((project(a) - project(b)) as nat, word() as nat);
    } else {
        lemma_mod_adds(project(a) - project(b), word(), word());
        lemma_small_mod((word() + project(a) - project(b)) as nat, word() as nat);
    }
}

proof fn projection_slot(n: nat, cap: nat)
    requires valid_capacity(cap)
    ensures project(n) % (cap as int) == n % cap
{
    lemma_fundamental_div_mod(word(), cap as int);
    lemma_mod_mod(n as int, cap as int, word() / (cap as int));
}

tracked struct Permissions<T> {
    token: RingProtocol::storage<T>,
    cells: Map<nat, cell::PointsTo<T>>,
}
struct PermissionPredicate;
impl<T> InvariantPredicate<RingProtocol::Instance<T>, Permissions<T>> for PermissionPredicate {
    closed spec fn inv(instance: RingProtocol::Instance<T>, data: Permissions<T>) -> bool {
        &&& data.token.instance_id() == instance.id()
        &&& data.token.value() == data.cells
    }
}

struct_with_invariants! {
    #[repr(align(64))]
    struct SharedRing<T, const CAP: usize> {
        buffer: Vec<PCell<T>>,
        head: AtomicUsize<_, RingProtocol::head<T>, _>,
        tail: AtomicUsize<_, RingProtocol::tail<T>, _>,
        producer_alive: AtomicBool<_, RingProtocol::producer_alive<T>, _>,
        consumer_alive: AtomicBool<_, RingProtocol::consumer_alive<T>, _>,
        instance: Tracked<RingProtocol::Instance<T>>,
        permissions: Tracked<AtomicInvariant<RingProtocol::Instance<T>, Permissions<T>, PermissionPredicate>>,
    }
    pub closed spec fn wf(&self) -> bool {
        predicate {
            &&& valid_capacity(CAP as nat)
            &&& self.buffer@.len() == CAP
            &&& self.instance@.cells().len() == CAP
            &&& self.permissions@.constant() == self.instance@
            &&& self.permissions@.namespace() != self.head.atomic_inv@.namespace()
            &&& self.permissions@.namespace() != self.tail.atomic_inv@.namespace()
            &&& forall|i: int| 0 <= i < CAP ==> #[trigger] self.instance@.cells()[i] == self.buffer@[i].id()
        }
        invariant on head with (instance) is (v: usize, g: RingProtocol::head<T>) {
            &&& g.instance_id() == instance@.id()
            &&& v == project(g.value())
        }
        invariant on tail with (instance) is (v: usize, g: RingProtocol::tail<T>) {
            &&& g.instance_id() == instance@.id()
            &&& v == project(g.value())
        }
        invariant on producer_alive with (instance) is (v: bool, g: RingProtocol::producer_alive<T>) {
            &&& g.instance_id() == instance@.id()
            &&& v == g.value()
        }
        invariant on consumer_alive with (instance) is (v: bool, g: RingProtocol::consumer_alive<T>) {
            &&& g.instance_id() == instance@.id()
            &&& v == g.value()
        }
    }
}

pub struct Producer<T, const CAP: usize> {
    shared: Arc<SharedRing<T, CAP>>,
    local_head: usize,
    shadow_tail: usize,
    writer: Tracked<RingProtocol::writer<T>>,
    marker: PhantomData<T>,
}
pub struct Consumer<T, const CAP: usize> {
    shared: Arc<SharedRing<T, CAP>>,
    local_tail: usize,
    shadow_head: usize,
    reader: Tracked<RingProtocol::reader<T>>,
    marker: PhantomData<T>,
}
impl<T, const CAP: usize> Producer<T, CAP> {
    #[verifier::type_invariant]
    pub closed spec fn wf(&self) -> bool {
        Self::parts_wf(&self.shared, self.local_head, self.shadow_tail, self.writer@)
    }
    closed spec fn parts_wf(shared: &SharedRing<T, CAP>, head: usize, shadow: usize, writer: RingProtocol::writer<T>) -> bool {
        &&& shared.wf()
        &&& writer.instance_id() == shared.instance@.id()
        &&& !writer.value().busy
        &&& !writer.value().closed
        &&& head == project(writer.value().pos)
        &&& shadow == project(writer.value().shadow)
        &&& writer.value().shadow <= writer.value().pos <= writer.value().shadow + CAP
        &&& writer.value().sent.len() == writer.value().pos
    }
    pub closed spec fn sent(&self) -> Seq<T> { self.writer@.value().sent }
    pub closed spec fn observed_free(&self) -> nat { (self.writer@.value().shadow + CAP - self.writer@.value().pos) as nat }
    pub closed spec fn observed_disconnect(&self) -> bool { self.writer@.value().consumer_closed }
    pub closed spec fn channel(&self) -> RingProtocol::Instance<T> { self.shared.instance@ }
}
impl<T, const CAP: usize> Consumer<T, CAP> {
    #[verifier::type_invariant]
    pub closed spec fn wf(&self) -> bool {
        &&& Self::parts_wf(&self.shared, self.local_tail, self.shadow_head, self.reader@)
        &&& !self.reader@.value().closing
    }
    closed spec fn parts_wf(shared: &SharedRing<T, CAP>, tail: usize, shadow: usize, reader: RingProtocol::reader<T>) -> bool {
        &&& shared.wf()
        &&& reader.instance_id() == shared.instance@.id()
        &&& !reader.value().busy
        &&& tail == project(reader.value().pos)
        &&& shadow == project(reader.value().shadow)
        &&& reader.value().pos <= reader.value().shadow <= reader.value().pos + CAP
        &&& reader.value().received.len() == reader.value().pos
    }
    pub closed spec fn received(&self) -> Seq<T> { self.reader@.value().received }
    pub closed spec fn observed_available(&self) -> nat { (self.reader@.value().shadow - self.reader@.value().pos) as nat }
    pub closed spec fn closed_and_drained(&self) -> bool { self.reader@.value().closed_head == Some(self.reader@.value().pos) }
    pub closed spec fn channel(&self) -> RingProtocol::Instance<T> { self.shared.instance@ }
}

pub fn spsc_channel<T, const CAP: usize>() -> (pc: (Producer<T, CAP>, Consumer<T, CAP>))
    requires valid_capacity(CAP as nat)
    ensures pc.0.wf(), pc.1.wf(), pc.0.channel() == pc.1.channel(),
        pc.0.sent() == Seq::<T>::empty(), pc.1.received() == Seq::<T>::empty()
{
    assert!(CAP >= 2, "capacity must be at least 2");
    assert!(CAP <= usize::MAX / 2 + 1, "capacity exceeds bounds");
    proof {
        lemma_small_mod(1, CAP as nat);
        lemma_mod_bound(usize::MAX as int, CAP as int);
        lemma_mod_adds(usize::MAX as int, 1, CAP as int);
        if usize::MAX as int % (CAP as int) + 1 < CAP {
            assert(false);
        }
    }
    assert!(usize::MAX % CAP == CAP - 1, "capacity must be a power of two");
    let mut buffer = Vec::<PCell<T>>::new();
    let tracked mut perms = Map::<nat, cell::PointsTo<T>>::tracked_empty();
    while buffer.len() < CAP
        invariant
            buffer@.len() <= CAP,
            forall|n: nat| n < buffer@.len() ==> #[trigger] perms.dom().contains(n)
                && perms[n].id() == buffer@[n as int].id() && perms[n].is_uninit(),
            forall|n: nat| #[trigger] perms.dom().contains(n) ==> n < buffer@.len(),
        decreases CAP - buffer.len()
    {
        let ghost n = buffer@.len();
        let (cell, Tracked(perm)) = PCell::empty();
        buffer.push(cell);
        proof { perms.tracked_insert(n, perm); }
    }
    let ghost ids = Seq::new(buffer@.len(), |i: int| buffer@[i].id());
    let tracked (Tracked(instance), Tracked(storage_token), Tracked(head_token), Tracked(tail_token),
        Tracked(writer_token), Tracked(reader_token), Tracked(alive_token), Tracked(consumer_alive_token)) =
        RingProtocol::Instance::initialize(ids, perms);
    let tracked_inst = Tracked(instance.clone());
    let head = AtomicUsize::new(Ghost(tracked_inst), 0, Tracked(head_token));
    let tail = AtomicUsize::new(Ghost(tracked_inst), 0, Tracked(tail_token));
    let ghost namespace = if head.atomic_inv@.namespace() >= tail.atomic_inv@.namespace() {
        head.atomic_inv@.namespace() + 1
    } else { tail.atomic_inv@.namespace() + 1 };
    let tracked permissions = AtomicInvariant::new(instance, Permissions { token: storage_token, cells: perms }, namespace);
    let shared = Arc::new(SharedRing {
        buffer, head, tail,
        producer_alive: AtomicBool::new(Ghost(tracked_inst), true, Tracked(alive_token)),
        consumer_alive: AtomicBool::new(Ghost(tracked_inst), true, Tracked(consumer_alive_token)),
        instance: Tracked(instance),
        permissions: Tracked(permissions),
    });
    let p = Producer { shared: shared.clone(), local_head: 0, shadow_tail: 0,
        writer: Tracked(writer_token), marker: PhantomData };
    let c = Consumer { shared, local_tail: 0, shadow_head: 0,
        reader: Tracked(reader_token), marker: PhantomData };
    (p, c)
}

impl<T, const CAP: usize> Producer<T, CAP> {
    pub fn push(&mut self, item: T) -> (result: Result<(), PushError<T>>)
        ensures
            final(self).wf(),
            final(self).channel() == old(self).channel(),
            match result {
                Ok(()) => final(self).sent() == old(self).sent().push(item),
                Err(PushError::Full(v)) => v == item && final(self).sent() == old(self).sent()
                    && final(self).observed_free() == 0,
                Err(PushError::Disconnected(v)) => v == item && final(self).sent() == old(self).sent()
                    && final(self).observed_disconnect(),
            }
    {
        proof { use_type_invariant(&*self); }
        Self::push_fields(&self.shared, &mut self.local_head, &mut self.shadow_tail, &mut self.writer, item)
    }
    fn push_fields(shared: &SharedRing<T, CAP>, local_head: &mut usize, shadow_tail: &mut usize,
        writer: &mut Tracked<RingProtocol::writer<T>>, item: T) -> (result: Result<(), PushError<T>>)
        requires Self::parts_wf(shared, *old(local_head), *old(shadow_tail), old(writer)@)
        ensures
            Self::parts_wf(shared, *final(local_head), *final(shadow_tail), final(writer)@),
            match result {
                Ok(()) => final(writer)@.value().sent == old(writer)@.value().sent.push(item),
                Err(PushError::Full(v)) => v == item && final(writer)@.value().sent == old(writer)@.value().sent
                    && final(writer)@.value().pos == final(writer)@.value().shadow + CAP,
                Err(PushError::Disconnected(v)) => v == item && final(writer)@.value().sent == old(writer)@.value().sent
                    && final(writer)@.value().consumer_closed,
            }
        no_unwind
    {
        let alive = atomic_with_ghost!(&shared.consumer_alive => load(); returning v; ghost g => {
            if !v { shared.instance.borrow().note_consumer_closed(writer.borrow_mut(), &g); }
        });
        if !alive { return Err(PushError::Disconnected(item)); }
        proof { projection_distance(writer@.value().pos, writer@.value().shadow); }
        let distance = local_head.wrapping_sub(*shadow_tail);
        if distance >= CAP {
            let actual_tail = atomic_with_ghost!(&shared.tail => load(); returning v; ghost g => {
                shared.instance.borrow().refresh_tail(&g, writer.borrow_mut());
            });
            *shadow_tail = actual_tail;
            let alive = atomic_with_ghost!(&shared.consumer_alive => load(); returning v; ghost g => {
                if !v { shared.instance.borrow().note_consumer_closed(writer.borrow_mut(), &g); }
            });
            if !alive { return Err(PushError::Disconnected(item)); }
            proof { projection_distance(writer@.value().pos, writer@.value().shadow); }
            if local_head.wrapping_sub(actual_tail) >= CAP {
                return Err(PushError::Full(item));
            }
        }
        let ghost pos = writer@.value().pos;
        let tracked mut perm: cell::PointsTo<T>;
        open_atomic_invariant!(shared.permissions.borrow() => data => {
            proof {
                shared.instance.borrow().produce_ready(&data.token, writer.borrow());
                perm = data.cells.tracked_remove(pos);
                shared.instance.borrow().produce_start(perm, &mut data.token, writer.borrow_mut());
            }
        });
        proof { projection_slot(pos, CAP as nat); }
        let slot = *local_head % CAP;
        shared.buffer[slot].put(Tracked(&mut perm), item);
        let next = local_head.wrapping_add(1);
        proof { projection_step(pos); }
        open_atomic_invariant!(shared.permissions.borrow() => data => {
            atomic_with_ghost!(&shared.head => store(next); ghost g => {
                shared.instance.borrow().produce_end(item, perm, &mut data.token, &mut g, writer.borrow_mut());
                data.cells.tracked_insert(pos, perm);
            });
        });
        *local_head = next;
        Ok(())
    }
    pub fn capacity(&self) -> (n: usize)
        ensures n == CAP
    { CAP }
    pub fn is_disconnected(&self) -> bool
    {
        proof { use_type_invariant(self); }
        !atomic_with_ghost!(&self.shared.consumer_alive => load(); ghost g => {})
    }
    pub fn len(&self) -> (n: usize)
        ensures n <= CAP
    {
        proof { use_type_invariant(self); }
        let ghost head = self.writer@.value().pos;
        let ghost tail;
        let t = atomic_with_ghost!(&self.shared.tail => load(); returning v; ghost g => {
            tail = g.value();
            self.shared.instance.borrow().inspect_writer_tail(&g, self.writer.borrow());
            projection_distance(head, tail);
        });
        self.local_head.wrapping_sub(t)
    }
    pub fn is_full(&self) -> bool
    { self.len() >= CAP }
}
impl<T, const CAP: usize> Consumer<T, CAP> {
    pub fn pop(&mut self) -> (result: Result<T, PopError>)
        ensures
            final(self).wf(),
            final(self).channel() == old(self).channel(),
            match result {
                Ok(v) => final(self).received() == old(self).received().push(v),
                Err(PopError::Empty) => final(self).received() == old(self).received()
                    && final(self).observed_available() == 0,
                Err(PopError::Disconnected) => final(self).received() == old(self).received()
                    && final(self).observed_available() == 0 && final(self).closed_and_drained(),
            }
    {
        proof { use_type_invariant(&*self); }
        Self::pop_fields(&self.shared, &mut self.local_tail, &mut self.shadow_head, &mut self.reader)
    }
    fn pop_fields(shared: &SharedRing<T, CAP>, local_tail: &mut usize, shadow_head: &mut usize,
        reader: &mut Tracked<RingProtocol::reader<T>>) -> (result: Result<T, PopError>)
        requires Self::parts_wf(shared, *old(local_tail), *old(shadow_head), old(reader)@)
        ensures
            Self::parts_wf(shared, *final(local_tail), *final(shadow_head), final(reader)@),
            final(reader)@.value().closing == old(reader)@.value().closing,
            match result {
                Ok(v) => final(reader)@.value().received == old(reader)@.value().received.push(v),
                Err(PopError::Empty) => final(reader)@.value().received == old(reader)@.value().received
                    && final(reader)@.value().pos == final(reader)@.value().shadow,
                Err(PopError::Disconnected) => final(reader)@.value().received == old(reader)@.value().received
                    && final(reader)@.value().pos == final(reader)@.value().shadow
                    && final(reader)@.value().closed_head == Some(final(reader)@.value().pos),
            }
        no_unwind
    {
        let tail = *local_tail;
        proof { projection_distance(reader@.value().shadow, reader@.value().pos); }
        if tail == *shadow_head {
            let actual_head = atomic_with_ghost!(&shared.head => load(); returning v; ghost g => {
                shared.instance.borrow().refresh_head(&g, reader.borrow_mut());
            });
            *shadow_head = actual_head;
            proof { projection_distance(reader@.value().shadow, reader@.value().pos); }
            if tail == actual_head {
                let alive = atomic_with_ghost!(&shared.producer_alive => load(); returning v; ghost g => {
                    if !v { shared.instance.borrow().note_producer_closed(reader.borrow_mut(), &g); }
                });
                if alive { return Err(PopError::Empty); }
                let actual_head = atomic_with_ghost!(&shared.head => load(); returning v; ghost g => {
                    shared.instance.borrow().refresh_head(&g, reader.borrow_mut());
                });
                *shadow_head = actual_head;
                proof { projection_distance(reader@.value().shadow, reader@.value().pos); }
                if tail == actual_head { return Err(PopError::Disconnected); }
            }
        }
        proof { projection_distance(reader@.value().shadow, reader@.value().pos); }
        let ghost pos = reader@.value().pos;
        let tracked mut perm: cell::PointsTo<T>;
        open_atomic_invariant!(shared.permissions.borrow() => data => {
            proof {
                shared.instance.borrow().consume_ready(&data.token, reader.borrow());
                perm = data.cells.tracked_remove(pos);
                shared.instance.borrow().consume_start(perm, &mut data.token, reader.borrow_mut());
            }
        });
        proof { projection_slot(pos, CAP as nat); }
        let slot = tail % CAP;
        let item = shared.buffer[slot].take(Tracked(&mut perm));
        let next = tail.wrapping_add(1);
        proof { projection_step(pos); }
        open_atomic_invariant!(shared.permissions.borrow() => data => {
            atomic_with_ghost!(&shared.tail => store(next); ghost g => {
                shared.instance.borrow().consume_end(perm, &mut data.token, &mut g, reader.borrow_mut());
                data.cells.tracked_insert(pos + CAP as nat, perm);
            });
        });
        *local_tail = next;
        Ok(item)
    }
    pub fn capacity(&self) -> (n: usize)
        ensures n == CAP
    { CAP }
    pub fn is_disconnected(&self) -> bool
    {
        proof { use_type_invariant(self); }
        !atomic_with_ghost!(&self.shared.producer_alive => load(); ghost g => {})
    }
    pub fn len(&self) -> (n: usize)
        ensures n <= CAP
    {
        proof { use_type_invariant(self); }
        let ghost tail = self.reader@.value().pos;
        let ghost head;
        let h = atomic_with_ghost!(&self.shared.head => load(); returning v; ghost g => {
            head = g.value();
            self.shared.instance.borrow().inspect_reader_head(&g, self.reader.borrow());
            projection_distance(head, tail);
        });
        h.wrapping_sub(self.local_tail)
    }
    pub fn is_empty(&self) -> bool
    { self.len() == 0 }
}

/// the received values are a prefix of successful pushes on the channel
pub fn fifo_prefix<T, const CAP: usize>(p: &Producer<T, CAP>, c: &Consumer<T, CAP>)
    requires p.channel() == c.channel()
    ensures c.received() == p.sent().take(c.received().len() as int)
{
    proof {
        use_type_invariant(p);
        use_type_invariant(c);
        p.shared.instance.borrow().inspect_fifo(p.writer.borrow(), c.reader.borrow());
    }
}

fn check_send<S: Send>() {}
fn endpoints_are_send<T: Send, const CAP: usize>() {
    check_send::<Producer<T, CAP>>();
    check_send::<Consumer<T, CAP>>();
}

pub struct ClosedProducer<T, const CAP: usize> {
    shared: Arc<SharedRing<T, CAP>>,
    writer: Tracked<RingProtocol::writer<T>>,
}
pub struct ClosedConsumer<T, const CAP: usize> {
    shared: Arc<SharedRing<T, CAP>>,
    reader: Tracked<RingProtocol::reader<T>>,
}
impl<T, const CAP: usize> ClosedProducer<T, CAP> {
    #[verifier::type_invariant]
    pub closed spec fn wf(&self) -> bool {
        &&& self.shared.wf()
        &&& self.writer@.instance_id() == self.shared.instance@.id()
        &&& self.writer@.value().closed
        &&& !self.writer@.value().busy
    }
    pub closed spec fn channel(&self) -> RingProtocol::Instance<T> { self.shared.instance@ }
}
impl<T, const CAP: usize> ClosedConsumer<T, CAP> {
    #[verifier::type_invariant]
    pub closed spec fn wf(&self) -> bool {
        &&& self.shared.wf()
        &&& self.reader@.instance_id() == self.shared.instance@.id()
        &&& self.reader@.value().closing
        &&& !self.reader@.value().busy
    }
    pub closed spec fn channel(&self) -> RingProtocol::Instance<T> { self.shared.instance@ }
}
impl<T, const CAP: usize> Producer<T, CAP> {
    pub fn close(self) -> (closed: ClosedProducer<T, CAP>)
        ensures closed.wf(), closed.channel() == self.channel()
        no_unwind
    {
        proof { use_type_invariant(&self); }
        let Producer { shared, local_head: _, shadow_tail: _, mut writer, marker: _ } = self;
        atomic_with_ghost!(&shared.producer_alive => store(false); ghost g => {
            shared.instance.borrow().close_producer(writer.borrow_mut(), &mut g);
        });
        ClosedProducer { shared, writer }
    }
}
impl<T, const CAP: usize> Consumer<T, CAP> {
    pub fn close(self) -> (closed: ClosedConsumer<T, CAP>)
        ensures closed.wf(), closed.channel() == self.channel()
        no_unwind
    {
        proof { use_type_invariant(&self); }
        let Consumer { shared, mut local_tail, mut shadow_head, mut reader, marker: _ } = self;
        atomic_with_ghost!(&shared.consumer_alive => store(false); ghost g => {
            shared.instance.borrow().close_consumer(reader.borrow_mut(), &mut g);
        });
        let mut drained = 0;
        while drained < CAP
            invariant
                Self::parts_wf(&shared, local_tail, shadow_head, reader@),
                reader@.value().closing,
                drained <= CAP,
            decreases CAP - drained
        {
            match Self::pop_fields(&shared, &mut local_tail, &mut shadow_head, &mut reader) {
                Ok(item) => { let _item = item; },
                Err(_) => break,
            }
            drained = drained + 1;
        }
        ClosedConsumer { shared, reader }
    }
}

// vstd specifies Rc::try_unwrap, but not its Arc counterpart.
#[verifier::external_body]
fn try_unwrap_arc<T>(v: Arc<T>) -> (result: Result<T, Arc<T>>)
    ensures match result { Ok(t) => t == *v, Err(e) => e == v }
    opens_invariants none
    no_unwind
{
    Arc::try_unwrap(v)
}

/// reclaim the shared ring after both endpoints close.
/// on unwrap failure, we return the closed handles so the caller can retry
pub fn close_channel<T, const CAP: usize>(p: ClosedProducer<T, CAP>, c: ClosedConsumer<T, CAP>)
    -> (result: Result<(), (ClosedProducer<T, CAP>, ClosedConsumer<T, CAP>)>)
    requires p.channel() == c.channel()
    ensures result matches Err((p2, c2)) ==> p2.channel() == p.channel() && c2.channel() == c.channel()
{
    proof { use_type_invariant(&p); use_type_invariant(&c); }
    let ClosedProducer { shared, writer } = p;
    let ClosedConsumer { shared: other, reader } = c;
    { let _other = other; }
    match try_unwrap_arc(shared) {
        Ok(ring) => {
            ring.close(writer, reader);
            Ok(())
        },
        Err(shared) => {
            let other = shared.clone();
            Err((ClosedProducer { shared, writer }, ClosedConsumer { shared: other, reader }))
        },
    }
}

impl<T, const CAP: usize> SharedRing<T, CAP> {
    fn close(self, writer: Tracked<RingProtocol::writer<T>>, reader: Tracked<RingProtocol::reader<T>>)
        requires self.wf(),
            writer@.instance_id() == self.instance@.id(),
            reader@.instance_id() == self.instance@.id(),
            writer@.value().closed, reader@.value().closing,
            !writer@.value().busy, !reader@.value().busy,
    {
        let SharedRing { buffer, head, tail, producer_alive: _, consumer_alive: _,
            instance, permissions: Tracked(permissions) } = self;
        let (head, Tracked(head_token)) = head.into_inner();
        let (tail, Tracked(tail_token)) = tail.into_inner();
        let tracked mut data = permissions.into_inner();
        proof {
            instance.borrow().cleanup_ready(&data.token, &head_token, &tail_token,
                writer.borrow(), reader.borrow());
            projection_distance(head_token.value(), tail_token.value());
        }
        let count = head.wrapping_sub(tail);
        let mut i = 0;
        while i < count
            invariant
                i <= count,
                count == head_token.value() - tail_token.value(),
                count <= CAP,
                valid_capacity(CAP as nat),
                buffer@.len() == CAP,
                instance@.cells().len() == CAP,
                forall|j: int| 0 <= j < CAP ==> #[trigger] instance@.cells()[j] == buffer@[j].id(),
                tail == project(tail_token.value()),
                forall|n: nat| tail_token.value() + i <= n < head_token.value() ==>
                    #[trigger] data.cells.dom().contains(n)
                    && data.cells[n].id() == instance@.cells()[(n % (CAP as nat)) as int]
                    && data.cells[n].is_init(),
            decreases count - i
        {
            let ghost pos = (tail_token.value() + i) as nat;
            proof {
                assert(data.cells.dom().contains(pos));
                assert(data.cells[pos].id() == instance@.cells()[(pos % (CAP as nat)) as int]);
                assert(data.cells[pos].is_init());
            }
            let tracked mut perm = data.cells.tracked_remove(pos);
            proof {
                lemma_mod_adds(tail_token.value() as int, i as int, word());
                lemma_small_mod(i as nat, word() as nat);
                projection_slot(pos, CAP as nat);
            }
            let physical = tail.wrapping_add(i);
            proof {
                assert(physical as int == project(pos));
            }
            let slot = physical % CAP;
            proof {
                assert(slot as int == pos % (CAP as nat));
                assert(perm.id() == instance@.cells()[slot as int]);
                assert(instance@.cells()[slot as int] == buffer@[slot as int].id());
            }
            let _item = buffer[slot].take(Tracked(&mut perm));
            i = i + 1;
        }
    }
}
}

// the no_unwind annotations above are mandatory in Verus because of unwinding hooks.
// kernel doesn't use any unwinding obviously. still,
#[cfg(not(panic = "abort"))]
compile_error!("the verified SPSC ring requires panic = abort");

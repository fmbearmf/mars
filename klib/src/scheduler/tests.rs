mod ready_pool;

use super::{Scheduler, StealEvent};

#[test]
fn steal_trace_reports_attributed_successful_steals() {
    let scheduler = Scheduler::new();
    scheduler.record_steal(3, 1, 42);
    scheduler.record_steal(5, 2, 99);

    let mut events = [StealEvent {
        sequence: 0,
        source_cpu: 0,
        destination_cpu: 0,
        thread_id: 0,
    }; 2];

    let (cursor, count) = scheduler.steal_events_since(0, &mut events);

    assert_eq!(cursor, 2);
    assert_eq!(count, 2);
    assert_eq!(events[0].source_cpu, 3);
    assert_eq!(events[0].destination_cpu, 1);
    assert_eq!(events[0].thread_id, 42);
    assert_eq!(events[1].sequence, 1);
    assert_eq!(events[1].source_cpu, 5);
    assert_eq!(events[1].destination_cpu, 2);
    assert_eq!(events[1].thread_id, 99);
}

#[test]
fn steal_trace_retains_bounded_recent_history_and_resumes_partial_reads() {
    let scheduler = Scheduler::new();
    for id in 0..260 {
        scheduler.record_steal(1, 2, id);
    }

    let mut first = [StealEvent {
        sequence: 0,
        source_cpu: 0,
        destination_cpu: 0,
        thread_id: 0,
    }; 4];

    let (cursor, count) = scheduler.steal_events_since(0, &mut first);

    assert_eq!(count, 4);
    assert_eq!(first[0].sequence, 4);
    assert_eq!(first[0].thread_id, 4);
    assert_eq!(first[3].sequence, 7);
    assert_eq!(cursor, 8);

    let mut rest = [first[0]; 252];
    let (end, count) = scheduler.steal_events_since(cursor, &mut rest);

    assert_eq!(count, 252);
    assert_eq!(end, 260);
    assert_eq!(rest[251].thread_id, 259);
}

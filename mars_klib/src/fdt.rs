use core::{cmp::max, fmt, str::from_utf8};

use crate::vm::MemoryRegion;

pub const FDT_MAGIC: u32 = 0xd00d_feed;
pub const FDT_BEGIN_NODE: u32 = 0x1;
pub const FDT_END_NODE: u32 = 0x2;
pub const FDT_PROP: u32 = 0x3;
pub const FDT_NOP: u32 = 0x4;
pub const FDT_END: u32 = 0x9;

const MAX_RESERVED: usize = 128;
const MAX_DEPTH: usize = 64;
const MAX_FRAGS_PER_REGION: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    BadMagic,
    BadStructure,
    BadVersion,
    TooManyReserves,
    TooManyFragments,
    UnexpectedToken(u32),
    InvalidFormat,
}

type Result<T> = core::result::Result<T, Error>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct Header {
    total_size: u32,
    offset_dt_struct: u32,
    offset_dt_strings: u32,
    offset_mem_rsvmap: u32,
    version: u32,
    last_compatible_version: u32,
    boot_cpuid_phys: u32,
    size_dt_strings: u32,
}

pub struct Fdt<'a> {
    blob: &'a [u8],
    header: Header,
    struct_block: &'a [u8],
    strings_block: &'a [u8],
    mem_rsvmap_offset: usize,
}

impl fmt::Debug for Fdt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fdt")
            .field("total_size", &self.header.total_size)
            .field("offset_dt_struct", &self.header.offset_dt_struct)
            .field("offset_dt_strings", &self.header.offset_dt_strings)
            .finish()
    }
}

impl<'a> Fdt<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < 40 {
            return Err(Error::Truncated);
        }

        let magic = be_u32(bytes, 0)?;
        if magic != FDT_MAGIC {
            return Err(Error::BadMagic);
        }

        let header = Header {
            total_size: be_u32(bytes, 4)?,
            offset_dt_struct: be_u32(bytes, 8)?,
            offset_dt_strings: be_u32(bytes, 12)?,
            offset_mem_rsvmap: be_u32(bytes, 16)?,
            version: be_u32(bytes, 20)?,
            last_compatible_version: be_u32(bytes, 24)?,
            boot_cpuid_phys: be_u32(bytes, 28)?,
            size_dt_strings: be_u32(bytes, 32)?,
        };

        let total_size = header.total_size as usize;
        if total_size > bytes.len() {
            return Err(Error::Truncated);
        }

        let struct_offset = header.offset_dt_struct as usize;
        let strings_offset = header.offset_dt_strings as usize;
        if struct_offset >= total_size || strings_offset >= total_size {
            return Err(Error::BadStructure);
        }

        let struct_block = &bytes[struct_offset..total_size];
        let sdtsz = header.size_dt_strings as usize;
        if strings_offset + sdtsz > total_size {
            return Err(Error::BadStructure);
        }
        let strings_block = &bytes[strings_offset..strings_offset + sdtsz];

        Ok(Fdt {
            blob: bytes,
            header,
            struct_block,
            strings_block,
            mem_rsvmap_offset: header.offset_mem_rsvmap as usize,
        })
    }

    pub unsafe fn from_addr(addr: usize) -> Result<Self> {
        let ptr = addr as *const u8;
        let magic = u32::from_be(*(ptr as *const u32));
        if magic != FDT_MAGIC {
            return Err(Error::BadMagic);
        }

        let total_size = u32::from_be(*(ptr.add(4) as *const u32)) as usize;
        let dtb_bytes: &[u8] = core::slice::from_raw_parts(ptr, total_size);

        Fdt::new(dtb_bytes)
    }

    pub fn mem_reserve_map(&self, out: &mut [MemoryRegion]) -> Result<usize> {
        let offset = self.header.offset_mem_rsvmap as usize;
        if offset == 0 {
            return Ok(0);
        }
        if offset + 16 > self.blob.len() {
            return Err(Error::Truncated);
        }
        let mut index = 0usize;
        let mut pos = offset;
        loop {
            if pos + 16 > self.blob.len() {
                return Err(Error::Truncated);
            }
            let a_h = be_u32(self.blob, pos)?;
            let a_l = be_u32(self.blob, pos + 4)?;
            let s_h = be_u32(self.blob, pos + 8)?;
            let s_l = be_u32(self.blob, pos + 12)?;

            pos += 16;

            let address = ((a_h as u64) << 32) | (a_l as u64);
            let size = ((s_h as u64) << 32) | (s_l as u64);

            if address == 0 && size == 0 {
                break;
            }

            if index >= out.len() {
                return Err(Error::TooManyReserves);
            }
            out[index] = MemoryRegion {
                base: address as usize,
                size: size as usize,
            };
            index += 1;
        }
        Ok(index)
    }

    pub fn usable_mem_regions(&self, out: &mut [MemoryRegion]) -> Result<usize> {
        let mut reserves: [MemoryRegion; MAX_RESERVED] =
            [MemoryRegion { base: 0, size: 0 }; MAX_RESERVED];

        let reserve_count = self.mem_reserve_map(&mut reserves)?;

        let mut address_cells_stack: [u32; MAX_DEPTH] = [0; MAX_DEPTH];
        let mut size_cells_stack: [u32; MAX_DEPTH] = [0; MAX_DEPTH];
        let mut depth: usize = 0;

        address_cells_stack[0] = 2;
        size_cells_stack[0] = 1;

        let mut result_count: usize = 0;
        let mut pos: usize = 0;
        let struct_len = self.struct_block.len();
        let mut frag_buffer: [MemoryRegion; MAX_FRAGS_PER_REGION] =
            [MemoryRegion { base: 0, size: 0 }; MAX_FRAGS_PER_REGION];
        let mut node_name_stack: [&str; MAX_DEPTH] = [""; MAX_DEPTH];

        while pos < struct_len {
            let token = be_u32(self.struct_block, pos).map_err(|_| Error::Truncated)?;
            pos += 4;

            match token {
                FDT_BEGIN_NODE => {
                    let start = pos;
                    let mut found = false;
                    let mut end = pos;
                    while end < struct_len {
                        if self.struct_block[end] == 0 {
                            found = true;
                            break;
                        }
                        end += 1;
                    }

                    if !found {
                        return Err(Error::BadStructure);
                    }

                    let name_slice = &self.struct_block[start..end];
                    let node_name = from_utf8(name_slice).unwrap_or("");

                    if depth + 1 >= MAX_DEPTH {
                        return Err(Error::BadStructure);
                    }
                    depth += 1;
                    node_name_stack[depth] = node_name;

                    pos = end + 1;
                    while (pos - start) % 4 != 0 {
                        if pos >= struct_len {
                            return Err(Error::Truncated);
                        }
                        pos += 1;
                    }

                    address_cells_stack[depth] = address_cells_stack[depth - 1];
                    size_cells_stack[depth] = size_cells_stack[depth - 1];
                }

                FDT_END_NODE => {
                    if depth == 0 {
                        return Err(Error::BadStructure);
                    }
                    depth -= 1;
                }

                FDT_PROP => {
                    if pos + 8 > struct_len {
                        return Err(Error::Truncated);
                    }

                    let prop_len = be_u32(self.struct_block, pos)? as usize;
                    let name_offset = be_u32(self.struct_block, pos + 4)? as usize;
                    pos += 8;

                    if name_offset >= self.strings_block.len() {
                        return Err(Error::BadStructure);
                    }
                    let mut name_end = name_offset;
                    while name_end < self.strings_block.len() && self.strings_block[name_end] != 0 {
                        name_end += 1;
                    }
                    if name_end >= self.strings_block.len() {
                        return Err(Error::BadStructure);
                    }
                    let prop_name =
                        from_utf8(&self.strings_block[name_offset..name_end]).unwrap_or("");

                    if pos + prop_len > struct_len {
                        return Err(Error::Truncated);
                    }
                    let prop_val = &self.struct_block[pos..pos + prop_len];

                    pos += prop_len;
                    while pos % 4 != 0 {
                        if pos >= struct_len {
                            return Err(Error::Truncated);
                        }
                        pos += 1;
                    }

                    if prop_name == "#address-cells" {
                        if prop_len >= 4 {
                            let val = be_u32(prop_val, 0).map_err(|_| Error::BadStructure)?;
                            address_cells_stack[depth] = val;
                        }
                    } else if prop_name == "#size-cells" {
                        if prop_len >= 4 {
                            let val = be_u32(prop_val, 0).map_err(|_| Error::BadStructure)?;
                            size_cells_stack[depth] = val;
                        }
                    } else {
                        if prop_name == "device_type" {
                            if prop_val == b"memory\0" || prop_val == b"memory" {
                                node_name_stack[depth] = "memory";
                            }
                        } else if prop_name == "reg" {
                            let node_name = node_name_stack[depth];
                            let is_mem_node = node_name == "memory"
                                || node_name == "memory\0"
                                || node_name.starts_with("memory");

                            if is_mem_node {
                                let parent_depth = if depth == 0 { 0 } else { depth - 1 };

                                let a_cells = address_cells_stack[parent_depth] as usize;
                                let s_cells = size_cells_stack[parent_depth] as usize;

                                let tuple_cells = a_cells + s_cells;
                                if tuple_cells == 0 {
                                    continue;
                                }

                                if prop_len % (tuple_cells * 4) != 0 {
                                    continue;
                                }

                                let tuples = prop_len / (tuple_cells * 4);
                                for t in 0..tuples {
                                    let base_i = t * (tuple_cells) * 4;
                                    let mut address: u128 = 0;

                                    for c in 0..a_cells {
                                        let offset = base_i + c * 4;
                                        let cell = be_u32(prop_val, offset)
                                            .map_err(|_| Error::BadStructure)?;
                                        address = (address << 32) | (cell as u128);
                                    }

                                    let mut size: u128 = 0;

                                    for c in 0..s_cells {
                                        let offset = base_i + (a_cells + c) * 4;
                                        let cell = be_u32(prop_val, offset)
                                            .map_err(|_| Error::BadStructure)?;
                                        size = (size << 32) | (cell as u128);
                                    }

                                    let base_u64 = if address > u64::MAX as u128 {
                                        u64::MAX
                                    } else {
                                        address as u64
                                    };

                                    let size_u64 = if size > u64::MAX as u128 {
                                        u64::MAX
                                    } else {
                                        size as u64
                                    };

                                    let nfrags = sub_reserved_info(
                                        MemoryRegion {
                                            base: base_u64 as usize,
                                            size: size_u64 as usize,
                                        },
                                        &reserves[..reserve_count],
                                        &mut frag_buffer,
                                    )?;

                                    for f in 0..nfrags {
                                        if result_count >= out.len() {
                                            return Err(Error::TooManyFragments);
                                        }

                                        out[result_count] = frag_buffer[f];
                                        result_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }

                FDT_NOP => {}

                FDT_END => {
                    break;
                }

                other => {
                    return Err(Error::UnexpectedToken(other));
                }
            }
        }

        sort_and_merge(out, result_count);

        Ok(result_count)
    }
}

fn sub_reserved_info(
    region: MemoryRegion,
    reserves: &[MemoryRegion],
    out_frags: &mut [MemoryRegion],
) -> Result<usize> {
    let mut frags: [MemoryRegion; MAX_FRAGS_PER_REGION] =
        [MemoryRegion { base: 0, size: 0 }; MAX_FRAGS_PER_REGION];

    let mut frag_count = 1usize;
    frags[0] = region;

    for r in reserves {
        if r.size == 0 {
            continue;
        }
        let r_start = r.base;
        let r_end = r.base.saturating_add(r.size);

        let mut i = 0usize;
        while i < frag_count {
            let f = frags[i];
            let f_start = f.base;
            let f_end = f.base.saturating_add(f.size);

            if f_end <= r_start || f_start >= r_end {
                i += 1;
                continue;
            }

            let mut new_count = frag_count;
            let mut temp: [MemoryRegion; MAX_FRAGS_PER_REGION] =
                [MemoryRegion { base: 0, size: 0 }; MAX_FRAGS_PER_REGION];

            let mut ti = 0usize;
            for j in 0..frag_count {
                if j == i {
                    continue;
                }

                temp[ti] = frags[j];
                ti += 1;
            }

            if r_start > f_start {
                let left_size = r_start - f_start;
                temp[ti] = MemoryRegion {
                    base: f_start,
                    size: left_size,
                };
                ti += 1;
            }

            if r_end < f_end {
                let right_base = r_end;
                let right_size = f_end - r_end;
                temp[ti] = MemoryRegion {
                    base: right_base,
                    size: right_size,
                };
                ti += 1;
            }

            if ti > MAX_FRAGS_PER_REGION {
                return Err(Error::TooManyFragments);
            }

            for k in 0..ti {
                frags[k] = temp[k];
            }

            frag_count = ti;
        }
    }

    if frag_count > out_frags.len() {
        return Err(Error::TooManyFragments);
    }

    for i in 0..frag_count {
        out_frags[i] = frags[i];
    }
    Ok(frag_count)
}

fn sort_and_merge(arr: &mut [MemoryRegion], count: usize) {
    if count <= 1 {
        return;
    }

    for i in 1..count {
        let key = arr[i];
        let mut j = i;
        while j > 0 && arr[j - 1].base > key.base {
            arr[j] = arr[j - 1];
            j -= 1;
        }
        arr[j] = key;
    }

    let mut dest = 0usize;
    for i in 0..count {
        if dest == 0 {
            arr[dest] = arr[i];
            dest = 1;
            continue;
        }

        let last = arr[dest - 1];
        if arr[i].base <= last.base.saturating_add(last.size) {
            let new_end = max(
                last.base.saturating_add(last.size),
                arr[i].base.saturating_add(arr[i].size),
            );
            arr[dest - 1].size = new_end - last.base;
        } else {
            arr[dest] = arr[i];
            dest += 1;
        }
    }

    for i in dest..count {
        arr[i] = MemoryRegion { base: 0, size: 0 };
    }
}

fn be_u32(data: &[u8], offset: usize) -> Result<u32> {
    if offset + 4 > data.len() {
        return Err(Error::Truncated);
    }
    let s: [u8; 4] = [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
    Ok(u32::from_be_bytes(s))
}

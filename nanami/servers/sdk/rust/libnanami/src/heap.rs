use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{RequestError, Word};

use crate::println;

#[global_allocator]
static GLOBAL_ALLOCATOR: ImplicitListAllocator = ImplicitListAllocator::new();

extern "C" {
    static __heap_start: u8;
    static __heap_end: u8;
}

pub fn linker_heap_range() -> (Word, Word) {
    unsafe {
        (
            (&__heap_start as *const u8) as Word,
            (&__heap_end as *const u8) as Word,
        )
    }
}

pub fn init_heap(size_bytes: Word) -> Result<(Word, Word), RequestError> {
    let (base, mapped_size) = crate::request_heap(size_bytes)?;

    unsafe {
        GLOBAL_ALLOCATOR.init(base as usize, mapped_size as usize);
    }

    Ok((base, mapped_size))
}

pub fn heap_stats() -> (Word, Word, Word) {
    GLOBAL_ALLOCATOR.stats()
}

struct HeapRegion {
    start: AtomicUsize,
    end: AtomicUsize,
    cookie: AtomicUsize,
}

impl HeapRegion {
    const fn new() -> Self {
        Self {
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
            cookie: AtomicUsize::new(0),
        }
    }

    fn clear(&self) {
        self.start.store(0, Ordering::Release);
        self.end.store(0, Ordering::Release);
        self.cookie.store(0, Ordering::Release);
    }
}

struct ImplicitListAllocator {
    regions: [HeapRegion; HEAP_REGION_COUNT_MAX],
    region_count: AtomicUsize,
    lock: AtomicBool,
}

const HEAP_DEBUG_LOG_ALLOC: bool = false;
const HEAP_DEBUG_LOG_FREE: bool = false;
const HEAP_ALLOCATOR_VERSION: usize = 0x20260606_0006;

const HEAP_DEBUG_COALESCE_FROM: bool = false;
const HEAP_DEBUG_COALESCE_PREVIOUS: bool = false;
const HEAP_REGION_COUNT_MAX: usize = 16;
const HEAP_GROW_CHUNK_SIZE: usize = 4 * 1024 * 1024;
const HEAP_REGION_TAIL_SLACK_SIZE: usize = 64 * 1024;

impl ImplicitListAllocator {
    const fn new() -> Self {
        Self {
            regions: [const { HeapRegion::new() }; HEAP_REGION_COUNT_MAX],
            region_count: AtomicUsize::new(0),
            lock: AtomicBool::new(false),
        }
    }

    unsafe fn init(&self, base: usize, size: usize) {
        self.lock();

        let mut index = 0;
        while index < HEAP_REGION_COUNT_MAX {
            self.regions[index].clear();
            index += 1;
        }
        self.region_count.store(0, Ordering::Release);

        if !self.add_region_locked(base, size) {
            println!("[heap:init] failed base={:#x} size={:#x}", base, size,);
        }

        self.unlock();
    }

    fn stats(&self) -> (Word, Word, Word) {
        self.lock();

        let mut used = 0usize;
        let mut free = 0usize;
        let mut total = 0usize;
        let count = self.region_count_locked();
        let mut region_index = 0;

        while region_index < count {
            if let Some((start, end)) = self.region_locked(region_index) {
                total = total.saturating_add(end.saturating_sub(start));

                let mut current = start;
                while current < end {
                    let Some((size, is_used)) = (unsafe { read_valid_block(current, end) }) else {
                        break;
                    };

                    if is_used {
                        used = used.saturating_add(size);
                    } else {
                        free = free.saturating_add(size);
                    }

                    let Some(next) = current.checked_add(size) else {
                        break;
                    };

                    if next <= current {
                        break;
                    }

                    current = next;
                }
            }

            region_index += 1;
        }

        self.unlock();

        (used as Word, free as Word, total as Word)
    }

    fn lock(&self) {
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    unsafe fn allocate(&self, layout: Layout) -> *mut u8 {
        let request_size = layout.size();

        if request_size == 0 {
            return layout.align() as *mut u8;
        }

        let align = layout.align().max(BLOCK_ALIGN);

        if !align.is_power_of_two() {
            return ptr::null_mut();
        }

        if HEAP_DEBUG_LOG_ALLOC {
            println!("[heap:alloc] req={:#x}", request_size);
        }

        let mut should_grow = false;
        let mut last_current = 0usize;
        let mut last_end = 0usize;

        self.lock();
        let result = self.allocate_locked(layout, &mut last_current, &mut last_end);
        if result.is_null() {
            should_grow = true;
        }
        self.unlock();

        if !result.is_null() {
            return result;
        }

        if should_grow {
            let grow_size = grow_size_for_layout(layout);
            match crate::request_heap(grow_size as Word) {
                Ok((base, mapped_size)) => {
                    self.lock();
                    let added = self.add_region_locked(base as usize, mapped_size as usize);
                    let result = if added {
                        self.allocate_locked(layout, &mut last_current, &mut last_end)
                    } else {
                        ptr::null_mut()
                    };
                    self.unlock();

                    if !result.is_null() {
                        return result;
                    }

                    if !added {
                        println!(
                            "[heap:alloc] region table full req={:#x} grow={:#x}",
                            request_size, grow_size,
                        );
                    }
                }
                Err(error) => {
                    println!(
                        "[heap:alloc] request_heap failed req={:#x} grow={:#x} status={:#x}",
                        request_size,
                        grow_size,
                        request_error_code(error),
                    );
                }
            }
        }

        let (largest_free_block, free_block_count, used_block_count) = self.debug_free_summary();

        println!(
            "[heap:alloc] failed req={:#x} last_current={:#x} end={:#x} largest_free={:#x} free_blocks={} used_blocks={}",
            request_size,
            last_current,
            last_end,
            largest_free_block,
            free_block_count,
            used_block_count,
        );

        ptr::null_mut()
    }

    unsafe fn allocate_locked(
        &self,
        layout: Layout,
        last_current: &mut usize,
        last_end: &mut usize,
    ) -> *mut u8 {
        let request_size = layout.size();
        let align = layout.align().max(BLOCK_ALIGN);
        let count = self.region_count_locked();
        let mut region_index = 0;

        while region_index < count {
            if let Some((start, end)) = self.region_locked(region_index) {
                let result =
                    self.allocate_in_region_locked(layout, start, end, last_current, last_end);

                if !result.is_null() {
                    if HEAP_DEBUG_LOG_ALLOC {
                        println!("[heap:alloc] ret={:?}", result);
                    }

                    return result;
                }
            }

            region_index += 1;
        }

        if count == 0 {
            println!(
                "[heap:alloc] no heap region v={:#x} req={:#x} align={:#x}",
                HEAP_ALLOCATOR_VERSION, request_size, align,
            );
        }

        ptr::null_mut()
    }

    unsafe fn allocate_in_region_locked(
        &self,
        layout: Layout,
        start: usize,
        end: usize,
        last_current: &mut usize,
        last_end: &mut usize,
    ) -> *mut u8 {
        let request_size = layout.size();
        let align = layout.align().max(BLOCK_ALIGN);
        let mut current = start;
        let mut prev_current = 0usize;
        let mut prev_size = 0usize;
        let mut prev_used = false;

        *last_end = end;

        while current < end {
            *last_current = current;

            if current < start || current % BLOCK_ALIGN != 0 {
                println!(
                    "[heap:alloc] traversal escaped req={:#x} start={:#x} end={:#x} current={:#x}",
                    request_size, start, end, current,
                );
                break;
            }

            let Some((block_size, is_used)) = read_valid_block(current, end) else {
                debug_dump_invalid_traversal(
                    "[heap:alloc]",
                    request_size,
                    start,
                    end,
                    current,
                    prev_current,
                    prev_size,
                    prev_used,
                );
                break;
            };

            let Some(block_end) = current.checked_add(block_size) else {
                println!(
                    "[heap:alloc] block_end overflow current={:#x} block_size={:#x}",
                    current, block_size,
                );
                break;
            };

            if !is_used {
                let Some(user_base) = current
                    .checked_add(BLOCK_HEADER_SIZE)
                    .and_then(|value| value.checked_add(BACKPTR_SIZE))
                else {
                    println!("[heap:alloc] user_base overflow current={:#x}", current);
                    break;
                };

                let Some(user) = checked_align_up(user_base, align) else {
                    println!(
                        "[heap:alloc] user align overflow user_base={:#x} align={:#x}",
                        user_base, align,
                    );
                    break;
                };

                let Some(used_end) = user
                    .checked_add(request_size)
                    .and_then(|value| checked_align_up(value, BLOCK_ALIGN))
                else {
                    println!(
                        "[heap:alloc] used_end overflow user={:#x} request_size={:#x}",
                        user, request_size,
                    );
                    break;
                };

                if user_base < current || user < user_base || used_end < user {
                    println!(
                        "[heap:alloc] fit arithmetic escaped req={:#x} start={:#x} end={:#x} current={:#x} block_size={:#x}",
                        request_size, start, end, current, block_size,
                    );
                    break;
                }

                if used_end <= block_end {
                    let fit_is_valid = current >= start
                        && current < end
                        && block_end <= end
                        && user_base >= current
                        && user_base <= block_end
                        && user >= user_base
                        && user < block_end
                        && used_end >= user
                        && used_end <= block_end
                        && used_end >= start
                        && used_end <= end;

                    if !fit_is_valid {
                        println!(
                            "[heap:alloc] bad fit v={:#x} req={:#x} align={:#x} start={:#x} end={:#x} current={:#x} block_size={:#x} block_end={:#x} user_base={:#x} user={:#x} used_end={:#x} suffix_size={:#x}",
                            HEAP_ALLOCATOR_VERSION,
                            request_size,
                            align,
                            start,
                            end,
                            current,
                            block_size,
                            block_end,
                            user_base,
                            user,
                            used_end,
                            block_end.saturating_sub(used_end),
                        );
                    } else {
                        let suffix_size = block_end - used_end;

                        let allocated_end = if suffix_size >= MIN_BLOCK_SIZE {
                            write_block(used_end, suffix_size, false, 0);

                            let Some((check_size, check_used)) = read_valid_block(used_end, end)
                            else {
                                println!(
                                    "[heap:alloc] suffix verify failed used_end={:#x} suffix_size={:#x}",
                                    used_end, suffix_size,
                                );
                                return ptr::null_mut();
                            };

                            if check_size != suffix_size || check_used {
                                println!(
                                    "[heap:alloc] suffix mismatch used_end={:#x} expect={:#x} got={:#x} used={}",
                                    used_end, suffix_size, check_size, check_used,
                                );
                                return ptr::null_mut();
                            }

                            used_end
                        } else {
                            block_end
                        };

                        if allocated_end <= current {
                            println!(
                                "[heap:alloc] allocated_end invalid current={:#x} allocated_end={:#x}",
                                current, allocated_end,
                            );
                            break;
                        }

                        write_block(current, allocated_end - current, true, request_size);

                        *((user - BACKPTR_SIZE) as *mut usize) = current;

                        return user as *mut u8;
                    }
                }
            }

            let Some(next) = current.checked_add(block_size) else {
                println!(
                    "[heap:alloc] next overflow current={:#x} block_size={:#x}",
                    current, block_size,
                );
                break;
            };

            if next <= current {
                debug_dump_bad_block("[heap:alloc:non-forward]", current, end);
                println!(
                    "[heap:alloc] non-forward progress current={:#x} block_size={:#x} next={:#x} start={:#x} end={:#x}",
                    current, block_size, next, start, end,
                );
                break;
            }

            prev_current = current;
            prev_size = block_size;
            prev_used = is_used;
            current = next;
        }

        ptr::null_mut()
    }

    unsafe fn validate_allocated_user_locked(&self, user: usize, request_size: usize) -> bool {
        let Some((start, end)) = self.find_region_containing_locked(user) else {
            return false;
        };

        if user < start + BLOCK_HEADER_SIZE + BACKPTR_SIZE || user >= end {
            return false;
        }

        let block_start = *((user - BACKPTR_SIZE) as *const usize);
        let Some(block_min_end) = block_start.checked_add(MIN_BLOCK_SIZE) else {
            return false;
        };

        if block_start < start
            || block_start >= end
            || block_start % BLOCK_ALIGN != 0
            || block_min_end > end
        {
            return false;
        }

        let Some((block_size, is_used)) = read_valid_block(block_start, end) else {
            return false;
        };
        if !is_used {
            return false;
        }

        let header = block_start as *const BlockHeader;
        let stored_request_size = (*header).request_size;
        let Some(block_end) = block_start.checked_add(block_size) else {
            return false;
        };
        let Some(user_end) = user.checked_add(stored_request_size) else {
            return false;
        };

        stored_request_size >= request_size
            && stored_request_size <= block_size
            && user_end <= block_end
    }

    fn debug_free_summary(&self) -> (usize, usize, usize) {
        self.lock();

        let mut largest_free_block = 0usize;
        let mut free_block_count = 0usize;
        let mut used_block_count = 0usize;
        let count = self.region_count_locked();
        let mut region_index = 0;

        while region_index < count {
            if let Some((start, end)) = self.region_locked(region_index) {
                let mut current = start;
                let mut prev_current = 0usize;
                let mut prev_size = 0usize;
                let mut prev_used = false;

                while current < end {
                    let Some((size, is_used)) = (unsafe { read_valid_block(current, end) }) else {
                        unsafe {
                            debug_dump_invalid_traversal(
                                "[heap:summary]",
                                0,
                                start,
                                end,
                                current,
                                prev_current,
                                prev_size,
                                prev_used,
                            );
                        }

                        break;
                    };

                    if is_used {
                        used_block_count += 1;
                    } else {
                        free_block_count += 1;

                        if size > largest_free_block {
                            largest_free_block = size;
                        }
                    }

                    let Some(next) = current.checked_add(size) else {
                        break;
                    };

                    if next <= current {
                        break;
                    }

                    prev_current = current;
                    prev_size = size;
                    prev_used = is_used;
                    current = next;
                }
            }

            region_index += 1;
        }

        self.unlock();

        (largest_free_block, free_block_count, used_block_count)
    }

    unsafe fn free(&self, user_ptr: *mut u8) {
        if user_ptr.is_null() {
            return;
        }

        self.lock();

        let user = user_ptr as usize;
        let Some((start, end)) = self.find_region_containing_locked(user) else {
            if HEAP_DEBUG_LOG_FREE {
                println!("[heap:free] reject user={:#x}", user);
            }

            self.unlock();
            return;
        };

        if user < start + BLOCK_HEADER_SIZE + BACKPTR_SIZE || user >= end {
            if HEAP_DEBUG_LOG_FREE {
                println!(
                    "[heap:free] reject user={:#x} start={:#x} end={:#x}",
                    user, start, end,
                );
            }

            self.unlock();
            return;
        }

        let block_start = *((user - BACKPTR_SIZE) as *const usize);

        let Some(block_min_end) = block_start.checked_add(MIN_BLOCK_SIZE) else {
            if HEAP_DEBUG_LOG_FREE {
                println!(
                    "[heap:free] backptr overflow user={:#x} block_start={:#x}",
                    user, block_start,
                );
            }

            self.unlock();
            return;
        };

        if block_start < start
            || block_start >= end
            || block_start % BLOCK_ALIGN != 0
            || block_min_end > end
        {
            if HEAP_DEBUG_LOG_FREE {
                println!(
                    "[heap:free] bad backptr user={:#x} block_start={:#x} start={:#x} end={:#x}",
                    user, block_start, start, end,
                );
            }

            self.unlock();
            return;
        }

        let Some((block_size, is_used)) = read_valid_block(block_start, end) else {
            if HEAP_DEBUG_LOG_FREE {
                println!(
                    "[heap:free] invalid block user={:#x} block_start={:#x}",
                    user, block_start,
                );
                debug_dump_bad_block("[heap:free]", block_start, end);
            }

            self.unlock();
            return;
        };

        if !is_used {
            if HEAP_DEBUG_LOG_FREE {
                println!(
                    "[heap:free] double free? user={:#x} block_start={:#x} size={:#x}",
                    user, block_start, block_size,
                );
            }

            self.unlock();
            return;
        }

        write_block(block_start, block_size, false, 0);

        if HEAP_DEBUG_COALESCE_FROM {
            self.coalesce_from(block_start, end);
        }

        if HEAP_DEBUG_COALESCE_PREVIOUS {
            self.coalesce_previous(block_start, start, end);
        }

        self.unlock();
    }

    unsafe fn coalesce_from(&self, block_start: usize, end: usize) {
        let Some((mut size, is_used)) = read_valid_block(block_start, end) else {
            return;
        };

        if is_used {
            return;
        }

        loop {
            let Some(next) = block_start.checked_add(size) else {
                println!(
                    "[heap:coalesce_from] next overflow block_start={:#x} size={:#x}",
                    block_start, size,
                );
                return;
            };

            if next >= end {
                break;
            }

            let Some((next_size, next_used)) = read_valid_block(next, end) else {
                break;
            };

            if next_used {
                break;
            }

            let Some(merged_size) = size.checked_add(next_size) else {
                println!(
                    "[heap:coalesce_from] size overflow size={:#x} next_size={:#x}",
                    size, next_size,
                );
                return;
            };

            let Some(merged_end) = block_start.checked_add(merged_size) else {
                println!(
                    "[heap:coalesce_from] merged_end overflow block_start={:#x} merged_size={:#x}",
                    block_start, merged_size,
                );
                return;
            };

            if merged_end > end {
                println!(
                    "[heap:coalesce_from] merged out of range block_start={:#x} merged_size={:#x} end={:#x}",
                    block_start,
                    merged_size,
                    end,
                );
                return;
            }

            size = merged_size;
        }

        write_block(block_start, size, false, 0);
    }

    unsafe fn coalesce_previous(&self, block_start: usize, start: usize, end: usize) {
        let mut current = start;

        while current < block_start {
            let Some((size, is_used)) = read_valid_block(current, end) else {
                println!(
                    "[heap:coalesce_previous] bad current={:#x} target={:#x}",
                    current, block_start,
                );
                debug_dump_bad_block("[heap:coalesce_previous]", current, end);
                return;
            };

            let Some(next) = current.checked_add(size) else {
                println!(
                    "[heap:coalesce_previous] next overflow current={:#x} size={:#x}",
                    current, size,
                );
                return;
            };

            if next == block_start {
                if !is_used {
                    self.coalesce_from(current, end);
                }

                return;
            }

            if next <= current {
                println!(
                    "[heap:coalesce_previous] non-forward progress current={:#x} next={:#x}",
                    current, next,
                );
                return;
            }

            current = next;
        }
    }

    unsafe fn add_region_locked(&self, base: usize, size: usize) -> bool {
        let start = align_up(base, BLOCK_ALIGN);
        let Some(raw_end) = base.checked_add(size) else {
            return false;
        };
        let mapped_end = align_down(raw_end, BLOCK_ALIGN);
        let end = if mapped_end.saturating_sub(start)
            >= MIN_BLOCK_SIZE.saturating_add(HEAP_REGION_TAIL_SLACK_SIZE)
        {
            align_down(mapped_end - HEAP_REGION_TAIL_SLACK_SIZE, BLOCK_ALIGN)
        } else {
            mapped_end
        };

        if !region_bounds_valid(start, end) || end.saturating_sub(start) < MIN_BLOCK_SIZE {
            println!(
                "[heap:region] invalid base={:#x} size={:#x} start={:#x} end={:#x}",
                base, size, start, end,
            );
            return false;
        }

        let count = self.region_count_locked();
        if count >= HEAP_REGION_COUNT_MAX {
            return false;
        }

        let mut index = 0;
        while index < count {
            if let Some((other_start, other_end)) = self.region_locked(index) {
                if start < other_end && other_start < end {
                    println!(
                        "[heap:region] overlap new=[{:#x}..{:#x}) old=[{:#x}..{:#x})",
                        start, end, other_start, other_end,
                    );
                    return false;
                }
            }
            index += 1;
        }

        write_block(start, end - start, false, 0);

        self.regions[count].start.store(start, Ordering::Release);
        self.regions[count].end.store(end, Ordering::Release);
        self.regions[count]
            .cookie
            .store(make_cookie(start, end), Ordering::Release);
        self.region_count.store(count + 1, Ordering::Release);

        true
    }

    fn region_count_locked(&self) -> usize {
        self.region_count
            .load(Ordering::Acquire)
            .min(HEAP_REGION_COUNT_MAX)
    }

    fn region_locked(&self, index: usize) -> Option<(usize, usize)> {
        if index >= HEAP_REGION_COUNT_MAX {
            return None;
        }

        let region = &self.regions[index];
        let start = region.start.load(Ordering::Acquire);
        let end = region.end.load(Ordering::Acquire);
        let cookie = region.cookie.load(Ordering::Acquire);

        if region_ready(start, end, cookie) {
            Some((start, end))
        } else {
            None
        }
    }

    fn find_region_containing_locked(&self, addr: usize) -> Option<(usize, usize)> {
        let count = self.region_count_locked();
        let mut index = 0;

        while index < count {
            if let Some((start, end)) = self.region_locked(index) {
                if addr >= start && addr < end {
                    return Some((start, end));
                }
            }

            index += 1;
        }

        None
    }
}

#[repr(C)]
struct BlockHeader {
    magic: usize,
    size: usize,
    used: usize,
    request_size: usize,
}

const BLOCK_ALIGN: usize = 16;
const BLOCK_HEADER_SIZE: usize = align_up_const(core::mem::size_of::<BlockHeader>(), BLOCK_ALIGN);
const BACKPTR_SIZE: usize = core::mem::size_of::<usize>();
const MIN_BLOCK_SIZE: usize = align_up_const(BLOCK_HEADER_SIZE + BACKPTR_SIZE + 1, BLOCK_ALIGN);
const BLOCK_MAGIC: usize = 0x4845_4150_424c_4b31;
const HEAP_COOKIE: usize = 0x4845_4150_4b4f_4954;
const MIN_HEAP_VADDR: usize = 0x0100_0000;

unsafe fn write_block(addr: usize, size: usize, used: bool, request_size: usize) {
    let header = addr as *mut BlockHeader;

    (*header).magic = BLOCK_MAGIC;
    (*header).size = size;
    (*header).used = if used { 1 } else { 0 };
    (*header).request_size = request_size;
}

unsafe fn read_valid_block(addr: usize, end: usize) -> Option<(usize, bool)> {
    if addr == end {
        return None;
    }

    let header_end = addr.checked_add(BLOCK_HEADER_SIZE)?;
    if addr % BLOCK_ALIGN != 0 || header_end > end {
        return None;
    }

    let header = addr as *const BlockHeader;

    if (*header).magic != BLOCK_MAGIC {
        return None;
    }

    let size = (*header).size;
    let used = (*header).used;
    let request_size = (*header).request_size;

    let block_end = addr.checked_add(size)?;
    if size < MIN_BLOCK_SIZE
        || size % BLOCK_ALIGN != 0
        || block_end > end
        || used > 1
        || (used == 0 && request_size != 0)
        || (used != 0 && request_size > size)
    {
        return None;
    }

    Some((size, used != 0))
}

unsafe fn debug_dump_bad_block(prefix: &str, addr: usize, end: usize) {
    println!("{} bad block current={:#x} end={:#x}", prefix, addr, end,);

    if addr == end {
        println!("{} reason=end", prefix);
        return;
    }

    if addr % BLOCK_ALIGN != 0 {
        println!("{} reason=unaligned", prefix);
        return;
    }

    let Some(header_end) = addr.checked_add(BLOCK_HEADER_SIZE) else {
        println!("{} reason=header-overflow", prefix);
        return;
    };

    if header_end > end {
        println!("{} reason=header-out-of-range", prefix);
        return;
    }

    let header = addr as *const BlockHeader;

    println!("{} magic={:#x}", prefix, (*header).magic);
    println!("{} size={:#x}", prefix, (*header).size);
    println!("{} used={:#x}", prefix, (*header).used);
    println!("{} req={:#x}", prefix, (*header).request_size);

    if (*header).magic != BLOCK_MAGIC {
        println!("{} reason=bad-magic", prefix);
        return;
    }

    let size = (*header).size;

    if size < MIN_BLOCK_SIZE {
        println!("{} reason=size-too-small", prefix);
        return;
    }

    if size % BLOCK_ALIGN != 0 {
        println!("{} reason=size-unaligned", prefix);
        return;
    }

    let Some(block_end) = addr.checked_add(size) else {
        println!("{} reason=block-overflow", prefix);
        return;
    };

    if block_end > end {
        println!("{} reason=block-out-of-range", prefix);
        return;
    }
}

unsafe fn debug_dump_invalid_traversal(
    prefix: &str,
    request_size: usize,
    start: usize,
    end: usize,
    current: usize,
    prev_current: usize,
    prev_size: usize,
    prev_used: bool,
) {
    let prev_end = prev_current.checked_add(prev_size).unwrap_or(usize::MAX);
    let header_in_range = current
        .checked_add(BLOCK_HEADER_SIZE)
        .map(|header_end| current % BLOCK_ALIGN == 0 && header_end <= end)
        .unwrap_or(false);

    let (magic, size, used, block_req) = if header_in_range {
        let header = current as *const BlockHeader;
        (
            (*header).magic,
            (*header).size,
            (*header).used,
            (*header).request_size,
        )
    } else {
        (0, 0, 0, 0)
    };

    let reason = if current == end {
        "end"
    } else if current % BLOCK_ALIGN != 0 {
        "unaligned"
    } else if current.checked_add(BLOCK_HEADER_SIZE).is_none() {
        "header-overflow"
    } else if !header_in_range {
        "header-out-of-range"
    } else if magic != BLOCK_MAGIC {
        "bad-magic"
    } else if size < MIN_BLOCK_SIZE {
        "size-too-small"
    } else if size % BLOCK_ALIGN != 0 {
        "size-unaligned"
    } else if current
        .checked_add(size)
        .map(|block_end| block_end > end)
        .unwrap_or(true)
    {
        "block-out-of-range"
    } else if used > 1 {
        "bad-used"
    } else if used == 0 && block_req != 0 {
        "free-request-size"
    } else if used != 0 && block_req > size {
        "used-request-size"
    } else {
        "unknown"
    };

    println!(
        "{} invalid traversal req={:#x} start={:#x} end={:#x} current={:#x} prev={:#x} prev_size={:#x} prev_end={:#x} prev_used={} reason={} hdr_ok={} magic={:#x} size={:#x} used={:#x} block_req={:#x}",
        prefix,
        request_size,
        start,
        end,
        current,
        prev_current,
        prev_size,
        prev_end,
        prev_used,
        reason,
        header_in_range,
        magic,
        size,
        used,
        block_req,
    );
}

fn checked_align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

fn grow_size_for_layout(layout: Layout) -> usize {
    let minimum = layout
        .size()
        .saturating_add(layout.align())
        .saturating_add(MIN_BLOCK_SIZE)
        .max(HEAP_GROW_CHUNK_SIZE);

    align_up(minimum, PAGE_SIZE)
}

fn make_cookie(start: usize, end: usize) -> usize {
    start.rotate_left(17) ^ end.rotate_right(11) ^ HEAP_COOKIE
}

fn region_bounds_valid(start: usize, end: usize) -> bool {
    start >= MIN_HEAP_VADDR && end > start && start % BLOCK_ALIGN == 0 && end % BLOCK_ALIGN == 0
}

fn region_ready(start: usize, end: usize, cookie: usize) -> bool {
    region_bounds_valid(start, end) && cookie == make_cookie(start, end)
}

fn request_error_code(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => 2,
        RequestError::Unsupported => 3,
        RequestError::Transport => 4,
        RequestError::Protocol => 5,
        RequestError::Status(status) => status,
    }
}

unsafe impl GlobalAlloc for ImplicitListAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = self.allocate(layout);
        if ptr.is_null() {
            return ptr;
        }

        let user = ptr as usize;
        self.lock();
        let valid = self.validate_allocated_user_locked(user, layout.size());
        self.unlock();

        if !valid {
            println!(
                "[heap:alloc] invalid return ptr={:#x} size={:#x} align={:#x}",
                user,
                layout.size(),
                layout.align(),
            );
            return ptr::null_mut();
        }

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.free(ptr);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
                return ptr::null_mut();
            };

            return self.allocate(new_layout);
        }

        if new_size == 0 {
            self.free(ptr);
            return ptr::null_mut();
        }

        self.lock();

        let user = ptr as usize;
        let Some((start, end)) = self.find_region_containing_locked(user) else {
            self.unlock();
            return ptr::null_mut();
        };

        if user < start + BLOCK_HEADER_SIZE + BACKPTR_SIZE || user >= end {
            self.unlock();
            return ptr::null_mut();
        }

        let block_start = *((user - BACKPTR_SIZE) as *const usize);
        let Some(block_min_end) = block_start.checked_add(MIN_BLOCK_SIZE) else {
            self.unlock();
            return ptr::null_mut();
        };

        if block_start < start
            || block_start >= end
            || block_start % BLOCK_ALIGN != 0
            || block_min_end > end
        {
            self.unlock();
            return ptr::null_mut();
        }

        let Some((block_size, is_used)) = read_valid_block(block_start, end) else {
            self.unlock();
            return ptr::null_mut();
        };

        if !is_used {
            self.unlock();
            return ptr::null_mut();
        }

        let old_layout_size = layout.size();
        let old_request_size = (*(block_start as *const BlockHeader)).request_size;
        let Some(block_end) = block_start.checked_add(block_size) else {
            self.unlock();
            return ptr::null_mut();
        };
        let Some(old_user_end) = user.checked_add(old_request_size) else {
            self.unlock();
            return ptr::null_mut();
        };
        let Some(old_layout_end) = user.checked_add(old_layout_size) else {
            self.unlock();
            return ptr::null_mut();
        };
        if old_request_size > block_size || old_user_end > block_end || old_layout_end > block_end {
            self.unlock();
            return ptr::null_mut();
        };

        if new_size <= old_layout_size {
            self.unlock();
            return ptr;
        }

        self.unlock();

        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return ptr::null_mut();
        };

        let new_ptr = self.allocate(new_layout);

        if new_ptr.is_null() {
            return ptr::null_mut();
        }

        self.lock();
        let new_user = new_ptr as usize;
        let new_is_valid = self.find_region_containing_locked(new_user).is_some();
        self.unlock();
        if !new_is_valid {
            println!(
                "[heap:realloc] invalid new ptr old={:#x} new={:#x} old_size={:#x} new_size={:#x}",
                user, new_user, old_layout_size, new_size,
            );
            return ptr::null_mut();
        }

        ptr::copy_nonoverlapping(
            ptr,
            new_ptr,
            old_layout_size.min(old_request_size).min(new_size),
        );

        self.free(ptr);

        new_ptr
    }
}

const PAGE_SIZE: usize = 4096;

fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

fn align_down(value: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    value & !(align - 1)
}

const fn align_up_const(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

const MAXIMUM_PROC_PER_GROUP: usize = 64;

#[cfg(feature = "numa")]
use std::{collections::HashMap, sync::OnceLock};

#[cfg(feature = "numa")]
static MAPPING: OnceLock<HashMap<usize, Vec<usize>>> = OnceLock::new();

#[cfg(all(feature = "numa", target_os = "linux"))]
fn mapping() -> HashMap<usize, Vec<usize>> {
    fn initialize() -> HashMap<usize, Vec<usize>> {
        let mut map = HashMap::new();

        let max_node = unsafe { api::numa_max_node() as usize };
        for node in 0..=max_node {
            let mask = unsafe { api::numa_allocate_cpumask() };
            unsafe { api::numa_node_to_cpus(node as i32, mask) };

            let mut cpus = Vec::new();
            for cpu in 0..libc::CPU_SETSIZE {
                if unsafe { api::numa_bitmask_isbitset(mask, cpu) } != 0 {
                    cpus.push(cpu as usize);
                }
            }

            unsafe { api::numa_bitmask_free(mask) };

            if !cpus.is_empty() {
                map.insert(node, cpus);
            }
        }

        map
    }

    MAPPING.get_or_init(initialize).clone()
}

#[cfg(all(feature = "numa", target_os = "windows"))]
fn mapping() -> HashMap<usize, Vec<usize>> {
    fn initialize() -> HashMap<usize, Vec<usize>> {
        let mut map = HashMap::new();

        let mut highest_node: u32 = 0;
        let result = unsafe { api::GetNumaHighestNodeNumber(&mut highest_node) };

        if result == 0 {
            return map;
        }

        let group_count = unsafe { api::GetActiveProcessorGroupCount() } as usize;

        for group in 0..group_count {
            let count = unsafe { api::GetActiveProcessorCount(group as u16) } as usize;
            for number in 0..count {
                let processor = api::PROCESSOR_NUMBER { Group: group as u16, Number: number as u8, Reserved: 0 };
                let mut node = 0u16;
                let ok = unsafe { api::GetNumaProcessorNodeEx(&processor, &mut node) };
                if ok == 0 || (node as u32) > highest_node {
                    continue;
                }

                let cpu = group * MAXIMUM_PROC_PER_GROUP + number;
                map.entry(node as usize).or_default().push(cpu);
            }
        }

        map.retain(|_, cpus| !cpus.is_empty());

        map
    }

    MAPPING.get_or_init(initialize).clone()
}

#[cfg(all(feature = "numa", target_os = "linux"))]
pub fn bind_thread(id: usize) {
    fn num_cpus() -> usize {
        mapping().values().map(|cpus| cpus.len()).sum()
    }

    let id = id % num_cpus();
    let node = mapping().iter().find_map(|(node, cpus)| cpus.contains(&id).then_some(*node)).unwrap_or(0);

    unsafe {
        api::numa_run_on_node(node as i32);
        api::numa_set_preferred(node as i32);
    }
}

#[cfg(all(feature = "numa", target_os = "windows"))]
pub fn bind_thread(id: usize) {
    let map = mapping();
    let num_cpus = map.values().map(|cpus| cpus.len()).sum::<usize>();
    if num_cpus == 0 {
        return;
    }

    let id = id % num_cpus;
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_unstable_by_key(|&(node, _)| node);

    let mut remaining = id;
    let mut cpu = None;

    for (_, cpus) in entries {
        if cpus.is_empty() {
            continue;
        }

        if remaining < cpus.len() {
            cpu = Some(cpus[remaining]);
            break;
        } else {
            remaining -= cpus.len();
        }
    }

    let cpu = match cpu {
        Some(cpu) => cpu,
        None => return,
    };

    let group = (cpu / MAXIMUM_PROC_PER_GROUP) as u16;
    let bit = cpu % MAXIMUM_PROC_PER_GROUP;
    let affinity = api::GROUP_AFFINITY { Mask: 1u64 << bit, Group: group, Reserved: [0; 3] };

    unsafe {
        let current_thread = api::GetCurrentThread();
        api::SetThreadGroupAffinity(current_thread, &affinity, std::ptr::null_mut());
    }
}

#[cfg(not(feature = "numa"))]
pub fn bind_thread(_id: usize) {
    // No-op when NUMA is disabled
}

/// Marker trait for types that can be safely replicated per NUMA node.
///
/// # Safety
///
/// Implementing `NumaValue` asserts that `T` may be replicated per NUMA node
/// and safely accessed concurrently (i.e., `&T` must be `Sync`).
pub unsafe trait NumaValue: Sync {}

pub struct NumaReplicator<T: NumaValue> {
    allocated: Vec<*mut T>,
}

unsafe impl<T: NumaValue> Send for NumaReplicator<T> {}
unsafe impl<T: NumaValue> Sync for NumaReplicator<T> {}

impl<T: NumaValue> NumaReplicator<T> {
    #[cfg(all(feature = "numa", target_os = "linux"))]
    pub unsafe fn new<S: Fn() -> T>(source: S) -> Self {
        if api::numa_available() < 0 {
            panic!("NUMA is not available on this system");
        }

        let mut allocated = Vec::new();
        let mut nodes = Vec::new();

        for (node, cpus) in mapping() {
            if cpus.is_empty() {
                continue;
            }

            let ptr = api::numa_alloc_onnode(std::mem::size_of::<T>(), node as i32);
            if ptr.is_null() {
                panic!("Failed to allocate memory on NUMA node {node}");
            }

            let tptr = ptr as *mut T;
            std::ptr::write(tptr, source());

            allocated.push(tptr);
            nodes.push(node);
        }

        Self { allocated }
    }

    #[cfg(all(feature = "numa", target_os = "windows"))]
    pub unsafe fn new<S: Fn() -> T>(source: S) -> Self {
        let mut allocated = Vec::new();

        for (node, cpus) in mapping() {
            if cpus.is_empty() {
                continue;
            }

            let ptr = api::VirtualAllocExNuma(
                api::GetCurrentProcess(),
                std::ptr::null_mut(),
                std::mem::size_of::<T>(),
                api::MEM_COMMIT | api::MEM_RESERVE,
                api::PAGE_READWRITE,
                node as u32,
            );

            if (ptr as *mut u8).is_null() {
                panic!("Failed to allocate memory on NUMA node {node}");
            }

            let tptr = ptr as *mut T;
            std::ptr::write(tptr, source());

            allocated.push(tptr);
        }

        Self { allocated }
    }

    #[cfg(not(feature = "numa"))]
    pub unsafe fn new<S: Fn() -> T>(source: S) -> Self {
        let layout = std::alloc::Layout::new::<T>();
        let ptr = std::alloc::alloc(layout) as *mut T;
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }

        std::ptr::write(ptr, source());

        Self { allocated: vec![ptr] }
    }

    #[cfg(all(feature = "numa", target_os = "linux"))]
    pub unsafe fn get(&self) -> &T {
        let cpu = libc::sched_getcpu();
        let node = api::numa_node_of_cpu(cpu);

        let index = mapping().iter().enumerate().find_map(|(i, (n, _))| (*n as i32 == node).then_some(i)).unwrap_or(0);
        &*self.allocated[index]
    }

    #[cfg(all(feature = "numa", target_os = "windows"))]
    pub unsafe fn get(&self) -> &T {
        let cpu = api::GetCurrentProcessorNumber() as usize;
        let node = mapping().iter().find_map(|(n, cpus)| cpus.contains(&cpu).then_some(*n)).unwrap_or(0);
        let index = mapping().iter().enumerate().find_map(|(i, (n, _))| (*n == node).then_some(i)).unwrap_or(0);
        &*self.allocated[index]
    }

    #[cfg(not(feature = "numa"))]
    pub unsafe fn get(&self) -> &T {
        &*self.allocated[0]
    }

    pub unsafe fn get_all(&self) -> Vec<&T> {
        self.allocated.iter().map(|&ptr| &*ptr).collect()
    }
}

impl<T: NumaValue> Drop for NumaReplicator<T> {
    fn drop(&mut self) {
        for &ptr in &self.allocated {
            unsafe {
                std::ptr::drop_in_place(ptr);

                #[cfg(all(feature = "numa", target_os = "linux"))]
                api::numa_free(ptr as *mut libc::c_void, std::mem::size_of::<T>());

                #[cfg(all(feature = "numa", target_os = "windows"))]
                api::VirtualFree(ptr as *mut std::ffi::c_void, 0, api::MEM_RELEASE);

                #[cfg(not(feature = "numa"))]
                {
                    let layout = std::alloc::Layout::new::<T>();
                    std::alloc::dealloc(ptr as *mut u8, layout);
                }
            }
        }
    }
}

#[allow(dead_code)]
#[cfg(all(feature = "numa", target_os = "linux"))]
mod api {
    use libc::{c_int, c_void, size_t};

    #[repr(C)]
    pub struct Bitmask {
        size: c_int,
        maskp: *mut u32,
    }

    #[link(name = "numa")]
    extern "C" {
        pub fn numa_available() -> c_int;
        pub fn numa_max_node() -> c_int;
        pub fn numa_node_of_cpu(cpu: c_int) -> c_int;

        pub fn numa_alloc_onnode(size: size_t, node: c_int) -> *mut c_void;
        pub fn numa_free(mem: *mut c_void, size: size_t);

        pub fn numa_run_on_node(node: i32) -> i32;
        pub fn numa_set_preferred(node: i32);

        pub fn numa_node_to_cpus(node: c_int, mask: *mut Bitmask) -> c_int;
        pub fn numa_allocate_cpumask() -> *mut Bitmask;
        pub fn numa_bitmask_free(mask: *mut Bitmask);
        pub fn numa_bitmask_isbitset(mask: *const Bitmask, n: c_int) -> c_int;
    }
}

#[allow(dead_code)]
#[cfg(all(feature = "numa", target_os = "windows"))]
mod api {
    use std::ffi::c_void;

    #[repr(C)]
    #[allow(non_snake_case)]
    pub struct GROUP_AFFINITY {
        pub Mask: u64,
        pub Group: u16,
        pub Reserved: [u16; 3],
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    pub struct PROCESSOR_NUMBER {
        pub Group: u16,
        pub Number: u8,
        pub Reserved: u8,
    }

    pub const MEM_COMMIT: u32 = 0x00001000;
    pub const MEM_RESERVE: u32 = 0x00002000;
    pub const MEM_RELEASE: u32 = 0x00008000;
    pub const PAGE_READWRITE: u32 = 0x04;

    extern "system" {
        pub fn GetNumaHighestNodeNumber(highest_node_number: *mut u32) -> i32;
        pub fn GetActiveProcessorGroupCount() -> u16;
        pub fn GetActiveProcessorCount(group_number: u16) -> u32;
        pub fn GetNumaProcessorNodeEx(processor: *const PROCESSOR_NUMBER, node_number: *mut u16) -> i32;
        pub fn GetCurrentProcessorNumber() -> u32;
        pub fn GetCurrentProcess() -> isize;
        pub fn VirtualAllocExNuma(
            process: isize, address: *mut c_void, size: usize, allocation_type: u32, protect: u32, preferred: u32,
        ) -> *mut c_void;
        pub fn VirtualFree(address: *mut c_void, size: usize, free_type: u32) -> i32;
        pub fn GetCurrentThread() -> isize;
        pub fn SetThreadGroupAffinity(
            thread: isize, group_affinity: *const GROUP_AFFINITY, previous_affinity: *mut GROUP_AFFINITY,
        ) -> i32;
    }
}

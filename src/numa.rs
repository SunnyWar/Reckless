use hwlocality::Topology;
use hwlocality::object::types::ObjectType;
/// Returns a Vec of (NUMA node index, Vec<logical CPU IDs>), sorted by node and efficiency.
pub fn logical_ids_by_numa_node() -> Vec<(usize, Vec<usize>)> {
    let mut result = Vec::new();
    if let Ok(topology) = Topology::new() {
        // Get all NUMA nodes (no ObjectFilter needed, returns iterator)
        let numa_nodes = topology.objects_with_type(ObjectType::NUMANode);
        // Get all cpu kinds, sorted by efficiency (P-cores first)
        let mut kinds: Vec<hwlocality::cpu::kind::CpuKind> =
            topology.cpu_kinds().map(|it| it.collect()).unwrap_or_default();
        kinds.reverse();

        for node in numa_nodes {
            // node.cpuset() returns Option<&CpuSet>
            if let Some(node_cpuset) = node.cpuset() {
                let node_cpuset = node_cpuset.clone();
                let mut logicals = Vec::new();
                for kind in &kinds {
                    // Intersect NUMA node cpuset with this kind's cpuset
                    for cpu in node_cpuset.iter_set() {
                        if kind.cpuset.is_set(cpu) {
                            logicals.push(cpu.into());
                        }
                    }
                }
                if !logicals.is_empty() {
                    if let Some(idx) = node.os_index() {
                        result.push((idx, logicals));
                    }
                }
            }
        }
    }
    result
}
#[cfg(feature = "numa")]
use std::{collections::HashMap, sync::OnceLock};

#[cfg(feature = "numa")]
static MAPPING: OnceLock<HashMap<usize, Vec<usize>>> = OnceLock::new();

#[cfg(feature = "numa")]
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

#[cfg(feature = "numa")]
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
    #[cfg(feature = "numa")]
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

    #[cfg(not(feature = "numa"))]
    pub unsafe fn new<S: Fn() -> T>(source: S) -> Self {
        let ptr = std::alloc::alloc(std::alloc::Layout::new::<T>()).cast::<T>();
        assert!(!ptr.is_null(), "Failed to allocate memory for NumaReplicator");

        std::ptr::write(ptr, source());

        Self { allocated: vec![ptr] }
    }

    #[cfg(feature = "numa")]
    pub unsafe fn get(&self) -> &T {
        let cpu = libc::sched_getcpu();
        let node = api::numa_node_of_cpu(cpu);

        let index = mapping().iter().enumerate().find_map(|(i, (n, _))| (*n as i32 == node).then_some(i)).unwrap_or(0);
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

                #[cfg(feature = "numa")]
                api::numa_free(ptr as *mut libc::c_void, std::mem::size_of::<T>());

                #[cfg(not(feature = "numa"))]
                std::alloc::dealloc(ptr.cast::<u8>(), std::alloc::Layout::new::<T>());
            }
        }
    }
}

#[allow(dead_code)]
#[cfg(feature = "numa")]
mod api {
    use libc::{c_int, c_void, size_t};

    #[repr(C)]
    pub struct Bitmask {
        size: c_int,
        maskp: *mut u32,
    }

    #[link(name = "numa")]
    unsafe extern "C" {
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

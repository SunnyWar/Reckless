//! Windows-specific NUMA detection and thread-affinity helpers.
//!
//! This module is intentionally small and private to the `numa` module.

use super::{NumaConfig, SYSTEM_THREADS};
use windows_sys::Win32::{Foundation::HANDLE, System::Threading::GetCurrentThread};

/// Windows GROUP_AFFINITY structure for thread affinity masks.
/// Defines processor group and affinity mask for NUMA-aware thread binding.
///
/// Note: This is defined locally because GROUP_AFFINITY is not exposed
/// by windows-sys. While the crate has extensive Win32 bindings,
/// NUMA-specific types are not included in the current version.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
#[allow(non_snake_case)]
pub struct GROUP_AFFINITY {
    pub Mask: u64,
    pub Group: u16,
    pub Reserved: [u16; 3],
}

// NUMA-specific functions not exposed in windows-sys.
// SAFETY: These are thread-safe Windows API calls with standard signatures.
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetNumaHighestNodeNumber(highest_node_number: *mut u32) -> i32;
    pub fn GetNumaNodeProcessorMaskEx(node: u16, processor_mask: *mut GROUP_AFFINITY) -> i32;
    pub fn SetThreadGroupAffinity(
        thread: HANDLE, group_affinity: *const GROUP_AFFINITY, previous_group_affinity: *mut GROUP_AFFINITY,
    ) -> i32;
}

/// Get the highest NUMA node number on the system.
pub(crate) fn get_highest_node() -> Option<u32> {
    let mut highest = 0u32;
    // SAFETY: GetNumaHighestNodeNumber is a thread-safe Windows API call.
    unsafe { if GetNumaHighestNodeNumber(&mut highest) != 0 { Some(highest) } else { None } }
}

/// Get the processor affinity mask for a NUMA node.
pub(crate) fn get_node_affinity(node: u16) -> Option<GROUP_AFFINITY> {
    let mut affinity = GROUP_AFFINITY::default();
    // SAFETY: GetNumaNodeProcessorMaskEx is a thread-safe Windows API call.
    unsafe { if GetNumaNodeProcessorMaskEx(node, &mut affinity) != 0 { Some(affinity) } else { None } }
}

/// Set thread affinity to a NUMA node's processors.
pub(crate) fn set_thread_affinity(affinity: &GROUP_AFFINITY) -> bool {
    // SAFETY: SetThreadGroupAffinity is a thread-safe Windows API call.
    // GetCurrentThread() returns a pseudo-handle that is always valid for the current thread.
    unsafe {
        let handle = GetCurrentThread();
        SetThreadGroupAffinity(handle, affinity, std::ptr::null_mut()) != 0
    }
}

pub(crate) fn bind_current_thread_to_numa_node(cfg: &NumaConfig, node: usize) {
    // Use the full node affinity mask if available.
    // We only store affinity for nodes with CPUs.
    if node < cfg.node_affinity.len() {
        let affinity = &cfg.node_affinity[node];
        #[cfg(debug_assertions)]
        {
            let success = set_thread_affinity(affinity);
            if !success {
                eprintln!("Warning: Failed to set thread group affinity for NUMA node {}", node);
            }
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = set_thread_affinity(affinity);
        }
    }
}

pub(crate) fn rebuild_affinity_for_kept_nodes(node_affinity: &mut Vec<GROUP_AFFINITY>, indices_to_keep: &[usize]) {
    *node_affinity = indices_to_keep
        .iter()
        .filter(|&&idx| idx < node_affinity.len())
        .map(|&idx| node_affinity[idx])
        .collect();
}

pub(crate) fn detect_windows_numa() -> NumaConfig {
    let mut cfg = NumaConfig::empty();

    let fallback = || {
        let mut cfg = NumaConfig::empty();
        for cpu in 0..*SYSTEM_THREADS {
            cfg.add_cpu_to_node(0, cpu);
        }
        cfg
    };

    let Some(highest_node) = get_highest_node() else {
        return fallback();
    };

    for node in 0..=highest_node {
        let Some(affinity) = get_node_affinity(node as u16) else {
            #[cfg(debug_assertions)]
            eprintln!("Warning: Failed to get processor mask for NUMA node {}", node);
            continue;
        };

        let mask = affinity.Mask;
        let group = affinity.Group as usize;

        for bit in 0..64 {
            if (mask & (1u64 << bit)) != 0 {
                let cpu = group * 64 + bit;
                cfg.add_cpu_to_node(node as usize, cpu);
            }
        }

        while cfg.node_affinity.len() <= node as usize {
            cfg.node_affinity.push(GROUP_AFFINITY::default());
        }
        cfg.node_affinity[node as usize] = affinity;
    }

    if cfg.nodes.is_empty() {
        return fallback();
    }

    debug_assert_eq!(
        cfg.nodes.len(),
        cfg.node_affinity.len(),
        "node_affinity length should match nodes length after detection"
    );

    cfg
}

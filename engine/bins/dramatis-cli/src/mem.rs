//! Resident memory, without a dependency.
//!
//! Two freeze conditions turn on this number, so it is read from the OS rather than
//! estimated. `getrusage` gives the peak; current resident size needs a platform call.

/// Current resident set size in bytes, or 0 if it cannot be determined.
pub fn resident_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        macos::resident().unwrap_or(0)
    }
    #[cfg(target_os = "linux")]
    {
        linux::resident().unwrap_or(0)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

/// Peak resident set size in bytes, from `getrusage`.
///
/// The peak matters as much as the current value: a build step that briefly doubles memory
/// still has to fit the budget, and an average would hide it.
pub fn peak_resident_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    if let Some(peak) = macos::peak() {
        return peak;
    }
    #[cfg(unix)]
    unsafe {
        let mut usage: libc_rusage = std::mem::zeroed();
        if getrusage(0, &mut usage) != 0 {
            return 0;
        }
        // Linux reports kilobytes, macOS bytes. Getting this wrong is a factor of 1024,
        // which would be obvious — but only if someone happened to look.
        #[cfg(target_os = "linux")]
        return usage.ru_maxrss as u64 * 1024;
        #[cfg(not(target_os = "linux"))]
        return usage.ru_maxrss as u64;
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(unix)]
#[repr(C)]
#[allow(non_camel_case_types)]
struct libc_rusage {
    ru_utime: [i64; 2],
    ru_stime: [i64; 2],
    ru_maxrss: i64,
    ru_ixrss: i64,
    ru_idrss: i64,
    ru_isrss: i64,
    ru_minflt: i64,
    ru_majflt: i64,
    ru_nswap: i64,
    ru_inblock: i64,
    ru_oublock: i64,
    ru_msgsnd: i64,
    ru_msgrcv: i64,
    ru_nsignals: i64,
    ru_nvcsw: i64,
    ru_nivcsw: i64,
}

#[cfg(unix)]
unsafe extern "C" {
    fn getrusage(who: i32, usage: *mut libc_rusage) -> i32;
}

#[cfg(target_os = "macos")]
mod macos {
    /// `mach_task_basic_info`. Note the field order differs from the older
    /// `task_basic_info_64` — `suspend_count` moved to the end — so the two structs are not
    /// interchangeable even though both start with sizes.
    #[repr(C)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [i32; 2],
        system_time: [i32; 2],
        policy: i32,
        suspend_count: i32,
    }

    /// `MACH_TASK_BASIC_INFO`, not `TASK_BASIC_INFO_64` (5).
    ///
    /// The older flavor still returns success on current kernels but reports
    /// `resident_size` as zero — so a reader using it gets a plausible-looking 0.0 MB and
    /// no error. Two freeze conditions turn on this number, which is why it was worth
    /// checking against a process that had just touched 60 MB rather than trusting the
    /// call's return code.
    const MACH_TASK_BASIC_INFO: i32 = 20;
    const COUNT: u32 =
        (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;

    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: i32, info: *mut MachTaskBasicInfo, count: *mut u32) -> i32;
    }

    fn query() -> Option<MachTaskBasicInfo> {
        unsafe {
            let mut info: MachTaskBasicInfo = std::mem::zeroed();
            let mut count = COUNT;
            if task_info(mach_task_self(), MACH_TASK_BASIC_INFO, &mut info, &mut count) != 0 {
                return None;
            }
            Some(info)
        }
    }

    pub fn resident() -> Option<u64> {
        query().map(|info| info.resident_size)
    }

    /// The kernel tracks the high-water mark itself, which is more trustworthy than
    /// `getrusage` here and needs no unit conversion.
    pub fn peak() -> Option<u64> {
        query().map(|info| info.resident_size_max)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    pub fn resident() -> Option<u64> {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
}

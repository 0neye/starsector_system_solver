#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io;
    use std::ptr;

    type Bool = i32;
    type Handle = *mut c_void;

    const CPU_SET_INFORMATION_TYPE_CPU_SET: u32 = 0;
    const SYSTEM_CPU_SET_ID_OFFSET: usize = 8;
    const SYSTEM_CPU_SET_EFFICIENCY_CLASS_OFFSET: usize = 18;
    const SYSTEM_CPU_SET_FLAGS_OFFSET: usize = 19;
    const CPU_SET_ALLOCATED_FLAG: u8 = 1 << 1;
    const CPU_SET_ALLOCATED_TO_TARGET_PROCESS_FLAG: u8 = 1 << 2;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn GetCurrentThread() -> Handle;
        fn GetSystemCpuSetInformation(
            information: *mut c_void,
            buffer_length: u32,
            returned_length: *mut u32,
            process: Handle,
            flags: u32,
        ) -> Bool;
        fn SetProcessDefaultCpuSets(
            process: Handle,
            cpu_set_ids: *const u32,
            cpu_set_id_count: u32,
        ) -> Bool;
        fn SetThreadSelectedCpuSets(
            thread: Handle,
            cpu_set_ids: *const u32,
            cpu_set_id_count: u32,
        ) -> Bool;
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct CpuSet {
        id: u32,
        efficiency_class: u8,
    }

    pub(super) fn prefer_performance_cores() {
        if let Err(err) = set_performance_cpu_sets() {
            eprintln!("warning: could not restrict solver to performance CPU cores: {err}");
        }
    }

    fn set_performance_cpu_sets() -> io::Result<()> {
        let cpu_sets = system_cpu_sets()?;
        let Some(ids) = performance_cpu_set_ids(&cpu_sets) else {
            return Ok(());
        };
        let count = u32::try_from(ids.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "too many CPU sets reported by Windows",
            )
        })?;

        unsafe {
            let process = GetCurrentProcess();
            if SetProcessDefaultCpuSets(process, ids.as_ptr(), count) == 0 {
                return Err(io::Error::last_os_error());
            }

            let thread = GetCurrentThread();
            if SetThreadSelectedCpuSets(thread, ids.as_ptr(), count) == 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(())
    }

    fn system_cpu_sets() -> io::Result<Vec<CpuSet>> {
        let mut returned_length = 0u32;
        unsafe {
            GetSystemCpuSetInformation(
                ptr::null_mut(),
                0,
                &mut returned_length,
                GetCurrentProcess(),
                0,
            );
        }
        if returned_length == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut buffer = vec![0u8; returned_length as usize];
        let ok = unsafe {
            GetSystemCpuSetInformation(
                buffer.as_mut_ptr().cast(),
                returned_length,
                &mut returned_length,
                GetCurrentProcess(),
                0,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }

        parse_cpu_sets(&buffer)
    }

    fn performance_cpu_set_ids(cpu_sets: &[CpuSet]) -> Option<Vec<u32>> {
        let max_efficiency_class = cpu_sets.iter().map(|cpu| cpu.efficiency_class).max()?;
        let has_multiple_classes = cpu_sets
            .iter()
            .any(|cpu| cpu.efficiency_class != max_efficiency_class);
        if !has_multiple_classes {
            return None;
        }

        let ids = cpu_sets
            .iter()
            .filter(|cpu| cpu.efficiency_class == max_efficiency_class)
            .map(|cpu| cpu.id)
            .collect::<Vec<_>>();
        (!ids.is_empty()).then_some(ids)
    }

    fn parse_cpu_sets(buffer: &[u8]) -> io::Result<Vec<CpuSet>> {
        let mut offset = 0usize;
        let mut cpu_sets = Vec::new();

        while offset < buffer.len() {
            let remaining = buffer.len() - offset;
            if remaining < 8 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated CPU set information header",
                ));
            }

            let size = read_u32(buffer, offset)? as usize;
            let record_type = read_u32(buffer, offset + 4)?;
            if size == 0 || size > remaining {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid CPU set information record size",
                ));
            }

            if record_type == CPU_SET_INFORMATION_TYPE_CPU_SET && size > SYSTEM_CPU_SET_FLAGS_OFFSET
            {
                let flags = buffer[offset + SYSTEM_CPU_SET_FLAGS_OFFSET];
                let allocated_elsewhere = flags & CPU_SET_ALLOCATED_FLAG != 0
                    && flags & CPU_SET_ALLOCATED_TO_TARGET_PROCESS_FLAG == 0;
                if !allocated_elsewhere {
                    cpu_sets.push(CpuSet {
                        id: read_u32(buffer, offset + SYSTEM_CPU_SET_ID_OFFSET)?,
                        efficiency_class: buffer[offset + SYSTEM_CPU_SET_EFFICIENCY_CLASS_OFFSET],
                    });
                }
            }

            offset += size;
        }

        Ok(cpu_sets)
    }

    fn read_u32(buffer: &[u8], offset: usize) -> io::Result<u32> {
        if offset + 4 > buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated CPU set information field",
            ));
        }

        Ok(u32::from_ne_bytes(
            buffer[offset..offset + 4]
                .try_into()
                .expect("slice length checked above"),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn selects_only_highest_efficiency_class() {
            let cpus = [
                CpuSet {
                    id: 10,
                    efficiency_class: 0,
                },
                CpuSet {
                    id: 11,
                    efficiency_class: 1,
                },
                CpuSet {
                    id: 12,
                    efficiency_class: 1,
                },
            ];

            assert_eq!(performance_cpu_set_ids(&cpus), Some(vec![11, 12]));
        }

        #[test]
        fn leaves_uniform_cpu_sets_unrestricted() {
            let cpus = [
                CpuSet {
                    id: 10,
                    efficiency_class: 0,
                },
                CpuSet {
                    id: 11,
                    efficiency_class: 0,
                },
            ];

            assert_eq!(performance_cpu_set_ids(&cpus), None);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn prefer_performance_cores() {}
}

pub(crate) fn prefer_performance_cores() {
    imp::prefer_performance_cores();
}

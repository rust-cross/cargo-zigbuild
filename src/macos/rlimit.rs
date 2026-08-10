use std::io;
use std::ptr;

fn nofile_limit() -> io::Result<libc::rlimit> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    // SAFETY: `limit` points to valid writable memory for the duration of the call.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(limit)
}

fn maxfiles_per_process() -> io::Result<libc::rlim_t> {
    let mut maxfiles: libc::c_int = 0;
    let mut size = std::mem::size_of_val(&maxfiles);

    // SAFETY: the output pointer and size describe `maxfiles`, and no new value is provided.
    if unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            ptr::from_mut(&mut maxfiles).cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }

    Ok(maxfiles as libc::rlim_t)
}

/// Raise the soft file descriptor limit to the maximum allowed by macOS.
pub(crate) fn raise_nofile_limit() -> io::Result<()> {
    let mut limit = nofile_limit()?;
    // The hard limit defaults to unlimited, while the actual ceiling is this sysctl value.
    let target = maxfiles_per_process()?.min(limit.rlim_max);

    if limit.rlim_cur >= target {
        return Ok(());
    }

    limit.rlim_cur = target;
    // SAFETY: `limit` is initialized and points to valid memory for the duration of the call.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{maxfiles_per_process, nofile_limit, raise_nofile_limit};

    #[test]
    fn raises_nofile_limit_to_system_maximum() {
        let before = nofile_limit().unwrap();
        let target = maxfiles_per_process().unwrap().min(before.rlim_max);

        raise_nofile_limit().unwrap();

        let after = nofile_limit().unwrap();
        assert_eq!(after.rlim_cur, before.rlim_cur.max(target));
        assert_eq!(after.rlim_max, before.rlim_max);
    }
}

//! Windows Job Object con `KILL_ON_JOB_CLOSE`.
//!
//! L'app inserisce se stessa in un job: tutti i processi che avvierà da quel
//! momento (yt-dlp, ffmpeg, spotDL, il WebView2 stesso) ne fanno parte. Quando
//! l'app termina, in qualunque modo (chiusura finestra, crash, kill dal task
//! manager), Windows chiude l'ultimo handle del job e termina in blocco tutti
//! i processi rimasti. Niente processi orfani in background.

#[cfg(windows)]
pub fn kill_children_on_exit() -> Result<(), String> {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: chiamate Win32 con puntatori validi per la durata della chiamata.
    // L'handle del job viene volutamente lasciato aperto: deve restare vivo
    // fino alla morte del processo, che è il momento in cui scatta il kill.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err("CreateJobObjectW fallita".into());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            return Err("SetInformationJobObject fallita".into());
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err("AssignProcessToJobObject fallita".into());
        }
    }
    Ok(())
}

/// Su sistemi non Windows (sviluppo e test) non fa nulla.
#[cfg(not(windows))]
pub fn kill_children_on_exit() -> Result<(), String> {
    Ok(())
}

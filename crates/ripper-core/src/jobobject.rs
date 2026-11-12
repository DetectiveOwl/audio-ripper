//! Windows Job Object con `KILL_ON_JOB_CLOSE`.
//!
//! The app puts itself into a job: every process it starts from then on
//! (yt-dlp, ffmpeg, spotDL, WebView2 itself) becomes part of it. When the app
//! ends, however that happens (window closed, crash, killed from Task
//! Manager), Windows closes the last handle of the job and terminates all the
//! remaining processes at once. No orphan processes are left in the background.

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

    // SAFETY: Win32 calls with pointers that are valid for the duration of the call.
    // The job handle is deliberately left open: it must stay alive until the
    // process dies, which is the moment the kill is triggered.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err("CreateJobObjectW failed".into());
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
            return Err("SetInformationJobObject failed".into());
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err("AssignProcessToJobObject failed".into());
        }
    }
    Ok(())
}

/// On non-Windows systems (development and tests) this does nothing.
#[cfg(not(windows))]
pub fn kill_children_on_exit() -> Result<(), String> {
    Ok(())
}

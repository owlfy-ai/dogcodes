use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use tokio::process::Child;
use windows::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, HANDLE},
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};
pub(super) struct Job(OwnedHandle);
fn failure(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}
fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: caller supplies a newly allocated, uniquely owned non-null handle.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}
impl Job {
    pub fn assign(child: &mut Child) -> io::Result<Self> {
        // SAFETY: unnamed private job and default non-inheritable security.
        let handle = unsafe { CreateJobObjectW(None, None) }.map_err(failure)?;
        let job = Self(owned(handle));
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let process = HANDLE(
            child
                .raw_handle()
                .ok_or_else(|| io::Error::other("missing process handle"))?,
        );
        // SAFETY: live handles and correctly sized documented limit structure.
        unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(std::mem::size_of_val(&limits)).map_err(io::Error::other)?,
            )
            .map_err(failure)?;
            AssignProcessToJobObject(handle, process).map_err(failure)?;
        }
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("missing process id"))?;
        // The process was created suspended. No native/user code can spawn a
        // grandchild before assignment to our non-breakaway job.
        // SAFETY: thread snapshot is newly allocated and uniquely owned.
        let snapshot =
            owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(failure)?);
        let mut entry = THREADENTRY32 {
            dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>())
                .map_err(io::Error::other)?,
            ..Default::default()
        };
        let mut count = 0;
        // SAFETY: valid snapshot and a correctly sized writable THREADENTRY32.
        let mut result = unsafe { Thread32First(HANDLE(snapshot.as_raw_handle()), &raw mut entry) };
        loop {
            match result {
                Ok(()) => {}
                Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
                Err(error) => return Err(failure(error)),
            }
            if entry.th32OwnerProcessID == pid {
                // SAFETY: access only the suspended child's enumerated thread.
                let thread = owned(
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                        .map_err(failure)?,
                );
                // SAFETY: thread handle remains live through synchronous resume.
                if unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                count += 1;
            }
            // SAFETY: same live snapshot and writable output as above.
            result = unsafe { Thread32Next(HANDLE(snapshot.as_raw_handle()), &raw mut entry) };
        }
        if count == 0 {
            return Err(io::Error::other("suspended child thread unavailable"));
        }
        Ok(job)
    }
    pub fn empty(&self) -> io::Result<bool> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: live private job handle and correctly sized writable structure.
        unsafe {
            QueryInformationJobObject(
                Some(HANDLE(self.0.as_raw_handle())),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                u32::try_from(std::mem::size_of_val(&info)).map_err(io::Error::other)?,
                None,
            )
        }
        .map_err(failure)?;
        Ok(info.ActiveProcesses == 0)
    }
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: the job is the private scope assigned before the child ran.
        unsafe { TerminateJobObject(HANDLE(self.0.as_raw_handle()), 1) }.map_err(failure)
    }
}

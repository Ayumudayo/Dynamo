$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'isolated-process-job: PowerShell 7.4 or newer is required'
}
if (-not $IsWindows) {
    throw 'isolated-process-job: Windows is required'
}

$script:ExpectedNativeSourceVersion = 'dynamo-isolated-job-native-2026-07-13-v5'
if ($null -eq ('Dynamo.Perf.Isolation.NativeLauncher' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

namespace Dynamo.Perf.Isolation
{
    public sealed class IsolationInputException : ArgumentException
    {
        public IsolationInputException(string code)
            : base("An isolated-process input failed validation.")
        {
            Code = code;
        }

        public string Phase => "ValidateInputs";
        public string Code { get; }
    }

    public sealed class IsolationLaunchCleanupException : AggregateException
    {
        internal IsolationLaunchCleanupException(
            Exception launchError,
            Exception cleanupError,
            IIsolationRecoveryProcess recoveryProcess)
            : base(
                "The isolated launch failed and process cleanup requires an explicit recovery retry.",
                launchError,
                cleanupError)
        {
            RecoveryProcess = recoveryProcess;
        }

        public IIsolationRecoveryProcess RecoveryProcess { get; }
    }

    internal sealed class SafeKernelHandle : SafeHandleZeroOrMinusOneIsInvalid
    {
        private SafeKernelHandle() : base(true) { }
        internal SafeKernelHandle(IntPtr handle, bool ownsHandle) : base(ownsHandle)
        {
            SetHandle(handle);
        }

        protected override bool ReleaseHandle()
        {
            return NativeMethods.CloseHandle(handle);
        }
    }

    internal sealed class PinnedPath : IDisposable
    {
        internal PinnedPath(
            SafeFileHandle handle,
            string expectedPath,
            string finalPath,
            string identity,
            bool isDirectory)
        {
            Handle = handle;
            ExpectedPath = expectedPath;
            FinalPath = finalPath;
            Identity = identity;
            IsDirectory = isDirectory;
        }

        internal SafeFileHandle Handle { get; private set; }
        internal string ExpectedPath { get; }
        internal string FinalPath { get; }
        internal string Identity { get; }
        internal bool IsDirectory { get; }

        public void Dispose()
        {
            if (Handle != null)
            {
                Handle.Dispose();
                Handle = null;
            }
        }
    }

    public sealed class ProcessEvidence
    {
        public int ProcessId { get; internal set; }
        public ulong CreationFileTimeUtc { get; internal set; }
        public bool IsProcessInJob { get; internal set; }
        public uint ActiveProcessCount { get; internal set; }
        public uint TotalProcessCount { get; internal set; }
        public uint TerminatedProcessCount { get; internal set; }
        public ulong[] ActiveProcessIds { get; internal set; } = Array.Empty<ulong>();
    }

    public sealed class ProcessWaitResult
    {
        public bool Exited { get; internal set; }
        public uint? ExitCode { get; internal set; }
    }

    public interface IIsolationRecoveryProcess : IDisposable
    {
        int ProcessId { get; }
        ulong CreationFileTimeUtc { get; }
        bool IsJobOpen { get; }
        ProcessEvidence GetEvidence();
        ProcessWaitResult Wait(int timeoutMilliseconds);
        void VerifyTerminatedIdentity();
        void Terminate(uint exitCode);
    }

    public sealed class IsolatedJobProcess : IIsolationRecoveryProcess
    {
        private readonly object sync = new object();
        private SafeKernelHandle jobHandle;
        private SafeKernelHandle processHandle;
        private bool disposed;

        internal IsolatedJobProcess(
            SafeKernelHandle job,
            SafeKernelHandle process,
            int processId,
            ulong creationFileTimeUtc)
        {
            jobHandle = job;
            processHandle = process;
            ProcessId = processId;
            CreationFileTimeUtc = creationFileTimeUtc;
        }

        public int ProcessId { get; }
        public ulong CreationFileTimeUtc { get; }
        public bool IsJobOpen
        {
            get
            {
                lock (sync)
                {
                    return !disposed && jobHandle != null && !jobHandle.IsClosed && !jobHandle.IsInvalid;
                }
            }
        }

        public ProcessEvidence GetEvidence()
        {
            lock (sync)
            {
                ThrowIfDisposed();
                if (jobHandle == null || jobHandle.IsClosed || jobHandle.IsInvalid)
                    throw new InvalidOperationException("The isolated job handle is closed.");

                return NativeLauncher.QueryEvidence(jobHandle, processHandle, ProcessId, CreationFileTimeUtc);
            }
        }

        public ProcessWaitResult Wait(int timeoutMilliseconds)
        {
            if (timeoutMilliseconds < 0)
                throw new ArgumentOutOfRangeException(nameof(timeoutMilliseconds));

            SafeKernelHandle waitHandle;
            bool referenceAdded = false;
            lock (sync)
            {
                ThrowIfDisposed();
                waitHandle = processHandle;
                waitHandle.DangerousAddRef(ref referenceAdded);
            }

            try
            {
                IntPtr rawWaitHandle = waitHandle.DangerousGetHandle();
                uint result = NativeMethods.WaitForSingleObjectRaw(rawWaitHandle, checked((uint)timeoutMilliseconds));
                if (result == NativeMethods.WAIT_FAILED)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "WaitForSingleObject failed.");
                if (result == NativeMethods.WAIT_TIMEOUT)
                    return new ProcessWaitResult { Exited = false, ExitCode = null };
                if (result != NativeMethods.WAIT_OBJECT_0)
                    throw new InvalidOperationException("WaitForSingleObject returned an unexpected result.");

                uint exitCode;
                if (!NativeMethods.GetExitCodeProcessRaw(rawWaitHandle, out exitCode))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "GetExitCodeProcess failed.");
                return new ProcessWaitResult { Exited = true, ExitCode = exitCode };
            }
            finally
            {
                if (referenceAdded) waitHandle.DangerousRelease();
            }
        }

        public Task<ProcessWaitResult> WaitAsync(int timeoutMilliseconds)
        {
            return Task.Run(() => Wait(timeoutMilliseconds));
        }

        public void VerifyTerminatedIdentity()
        {
            lock (sync)
            {
                ThrowIfDisposed();
                if (NativeMethods.WaitForSingleObject(processHandle, 0) != NativeMethods.WAIT_OBJECT_0)
                    throw new InvalidOperationException("The parent process has not terminated.");
                if (NativeLauncher.ReadCreationTime(processHandle) != CreationFileTimeUtc)
                    throw new InvalidOperationException("The parent process creation identity changed.");
            }
        }

        public void Terminate(uint exitCode)
        {
            lock (sync)
            {
                ThrowIfDisposed();
                if (jobHandle == null || jobHandle.IsClosed || jobHandle.IsInvalid)
                    throw new InvalidOperationException("The isolated job handle is closed.");
                if (!NativeMethods.TerminateJobObject(jobHandle, exitCode))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "TerminateJobObject failed.");
            }
        }

        public void Dispose()
        {
            lock (sync)
            {
                if (disposed) return;
                if (processHandle == null || processHandle.IsClosed || processHandle.IsInvalid ||
                    NativeMethods.WaitForSingleObject(processHandle, 0) != NativeMethods.WAIT_OBJECT_0)
                    throw new InvalidOperationException("Process termination was not verified; handles were preserved.");
                if (jobHandle == null || jobHandle.IsClosed || jobHandle.IsInvalid)
                    throw new InvalidOperationException("The open Job verification handle is unavailable; process handles were preserved.");
                ProcessEvidence evidence = NativeLauncher.QueryEvidence(
                    jobHandle, processHandle, ProcessId, CreationFileTimeUtc);
                if (evidence.ActiveProcessCount != 0 || evidence.ActiveProcessIds.Length != 0)
                    throw new InvalidOperationException("Job member termination was not verified; handles were preserved.");
                disposed = true;
                if (jobHandle != null)
                {
                    jobHandle.Dispose();
                    jobHandle = null;
                }
                if (processHandle != null)
                {
                    processHandle.Dispose();
                    processHandle = null;
                }
            }
            GC.SuppressFinalize(this);
        }

        private void ThrowIfDisposed()
        {
            if (disposed || processHandle == null || processHandle.IsClosed || processHandle.IsInvalid)
                throw new ObjectDisposedException(nameof(IsolatedJobProcess));
        }
    }

    public sealed class FailedLaunchRecoveryProcess : IIsolationRecoveryProcess
    {
        private readonly object sync = new object();
        private SafeKernelHandle jobHandle;
        private SafeKernelHandle processHandle;
        private bool disposed;

        internal FailedLaunchRecoveryProcess(
            SafeKernelHandle job,
            SafeKernelHandle process,
            int processId,
            ulong creationFileTimeUtc)
        {
            jobHandle = job;
            processHandle = process;
            ProcessId = processId;
            CreationFileTimeUtc = creationFileTimeUtc;
        }

        public int ProcessId { get; }
        public ulong CreationFileTimeUtc { get; }
        public bool IsJobOpen
        {
            get
            {
                lock (sync)
                {
                    return !disposed && jobHandle != null && !jobHandle.IsClosed && !jobHandle.IsInvalid;
                }
            }
        }

        public ProcessEvidence GetEvidence()
        {
            lock (sync)
            {
                ThrowIfDisposed();
                if (jobHandle == null || jobHandle.IsClosed || jobHandle.IsInvalid)
                    throw new InvalidOperationException("The failed launch was not assigned to a Job Object.");
                return NativeLauncher.QueryEvidence(
                    jobHandle, processHandle, ProcessId, CreationFileTimeUtc);
            }
        }

        public ProcessWaitResult Wait(int timeoutMilliseconds)
        {
            if (timeoutMilliseconds < 0)
                throw new ArgumentOutOfRangeException(nameof(timeoutMilliseconds));

            SafeKernelHandle waitHandle;
            bool referenceAdded = false;
            lock (sync)
            {
                ThrowIfDisposed();
                waitHandle = processHandle;
                waitHandle.DangerousAddRef(ref referenceAdded);
            }
            try
            {
                IntPtr rawWaitHandle = waitHandle.DangerousGetHandle();
                uint result = NativeMethods.WaitForSingleObjectRaw(
                    rawWaitHandle, checked((uint)timeoutMilliseconds));
                if (result == NativeMethods.WAIT_FAILED)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "WaitForSingleObject failed.");
                if (result == NativeMethods.WAIT_TIMEOUT)
                    return new ProcessWaitResult { Exited = false, ExitCode = null };
                if (result != NativeMethods.WAIT_OBJECT_0)
                    throw new InvalidOperationException("WaitForSingleObject returned an unexpected result.");
                uint exitCode;
                if (!NativeMethods.GetExitCodeProcessRaw(rawWaitHandle, out exitCode))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "GetExitCodeProcess failed.");
                return new ProcessWaitResult { Exited = true, ExitCode = exitCode };
            }
            finally
            {
                if (referenceAdded) waitHandle.DangerousRelease();
            }
        }

        public void VerifyTerminatedIdentity()
        {
            lock (sync)
            {
                ThrowIfDisposed();
                if (NativeMethods.WaitForSingleObject(processHandle, 0) != NativeMethods.WAIT_OBJECT_0)
                    throw new InvalidOperationException("The failed-launch process has not terminated.");
                if (CreationFileTimeUtc != 0 &&
                    NativeLauncher.ReadCreationTime(processHandle) != CreationFileTimeUtc)
                    throw new InvalidOperationException("The failed-launch process identity changed.");
            }
        }

        public void Terminate(uint exitCode)
        {
            lock (sync)
            {
                ThrowIfDisposed();
                if (jobHandle != null && !jobHandle.IsClosed && !jobHandle.IsInvalid)
                {
                    if (!NativeMethods.TerminateJobObject(jobHandle, exitCode))
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "TerminateJobObject failed.");
                }
                else
                {
                    if (NativeMethods.WaitForSingleObject(processHandle, 0) == NativeMethods.WAIT_OBJECT_0)
                        return;
                    if (!NativeMethods.TerminateProcess(processHandle, exitCode))
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "TerminateProcess failed.");
                }
            }
        }

        public void Dispose()
        {
            lock (sync)
            {
                if (disposed) return;
                if (processHandle == null || processHandle.IsClosed || processHandle.IsInvalid ||
                    NativeMethods.WaitForSingleObject(processHandle, 0) != NativeMethods.WAIT_OBJECT_0)
                    throw new InvalidOperationException(
                        "Failed-launch process termination was not verified; handles were preserved.");
                if (jobHandle != null && !jobHandle.IsClosed && !jobHandle.IsInvalid)
                {
                    ProcessEvidence evidence = NativeLauncher.QueryEvidence(
                        jobHandle, processHandle, ProcessId, CreationFileTimeUtc);
                    if (evidence.ActiveProcessCount != 0 || evidence.ActiveProcessIds.Length != 0)
                        throw new InvalidOperationException(
                            "Failed-launch Job termination was not verified; handles were preserved.");
                }
                disposed = true;
                CloseOwnedHandles();
            }
            GC.SuppressFinalize(this);
        }

        ~FailedLaunchRecoveryProcess()
        {
            lock (sync)
            {
                if (disposed) return;
                try
                {
                    if (processHandle != null && !processHandle.IsClosed && !processHandle.IsInvalid &&
                        NativeMethods.WaitForSingleObject(processHandle, 0) != NativeMethods.WAIT_OBJECT_0)
                    {
                        if (jobHandle != null && !jobHandle.IsClosed && !jobHandle.IsInvalid)
                            NativeMethods.TerminateJobObject(jobHandle, 0xE0010004);
                        else
                            NativeMethods.TerminateProcess(processHandle, 0xE0010005);
                    }
                }
                catch { }
                finally
                {
                    disposed = true;
                    CloseOwnedHandles();
                }
            }
        }

        private void CloseOwnedHandles()
        {
            if (jobHandle != null)
            {
                jobHandle.Dispose();
                jobHandle = null;
            }
            if (processHandle != null)
            {
                processHandle.Dispose();
                processHandle = null;
            }
        }

        private void ThrowIfDisposed()
        {
            if (disposed || processHandle == null || processHandle.IsClosed || processHandle.IsInvalid)
                throw new ObjectDisposedException(nameof(FailedLaunchRecoveryProcess));
        }
    }

    public static class NativeLauncher
    {
        public const string SourceVersion = "dynamo-isolated-job-native-2026-07-13-v5";
        private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
        private const uint CREATE_SUSPENDED = 0x00000004;
        private const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;
        private const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
        private const uint CREATE_NO_WINDOW = 0x08000000;
        private const uint STARTF_USESTDHANDLES = 0x00000100;
        private const uint GENERIC_READ = 0x80000000;
        private const uint GENERIC_WRITE = 0x40000000;
        private const uint FILE_READ_ATTRIBUTES = 0x00000080;
        private const uint FILE_SHARE_READ = 0x00000001;
        private const uint FILE_SHARE_WRITE = 0x00000002;
        private const uint OPEN_EXISTING = 3;
        private const uint CREATE_NEW = 1;
        private const uint FILE_ATTRIBUTE_NORMAL = 0x00000080;
        private const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
        private const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
        private const int JobObjectBasicAccountingInformation = 1;
        private const int JobObjectBasicProcessIdList = 3;
        private const int JobObjectExtendedLimitInformation = 9;
        private static readonly IntPtr PROC_THREAD_ATTRIBUTE_HANDLE_LIST = new IntPtr(0x00020002);
        private static readonly Regex EnvironmentKey = new Regex(
            @"^[A-Za-z_][A-Za-z0-9_.()]*$",
            RegexOptions.CultureInvariant | RegexOptions.Compiled);
        // Contract-only failpoints are private and consumed atomically via reflection
        // from the fresh-process contract. They are not part of the module ABI.
        private static string contractFailurePoint;

        public static IsolatedJobProcess Start(
            string executablePath,
            IReadOnlyList<string> argumentList,
            string workingDirectory,
            IReadOnlyDictionary<string, string> environment,
            string standardOutputPath,
            string standardErrorPath)
        {
            string executable = ValidateExactAbsolutePath(executablePath, "ExecutablePath");
            string working = ValidateExactAbsolutePath(workingDirectory, "WorkingDirectory");
            string stdout = ValidateOutputPathLexical(standardOutputPath, "StandardOutputPath");
            string stderr = ValidateOutputPathLexical(standardErrorPath, "StandardErrorPath");
            if (!String.Equals(Path.GetDirectoryName(stdout), Path.GetDirectoryName(stderr), StringComparison.OrdinalIgnoreCase))
                throw new IsolationInputException("OutputParentsMustMatch");
            if (String.Equals(stdout, stderr, StringComparison.OrdinalIgnoreCase))
                throw new IsolationInputException("OutputPathsMustDiffer");

            string commandLine = BuildCommandLine(executable, argumentList);
            byte[] environmentBlock = BuildEnvironmentBlock(environment);
            string failurePoint = Interlocked.Exchange(ref contractFailurePoint, null);
            string outputParent = Path.GetDirectoryName(stdout);
            PinnedPath executablePin = null;
            PinnedPath workingPin = null;
            PinnedPath outputParentPin = null;
            SafeKernelHandle job = null;
            SafeKernelHandle process = null;
            SafeKernelHandle thread = null;
            SafeFileHandle stdoutHandle = null;
            SafeFileHandle stderrHandle = null;
            SafeFileHandle stdinHandle = null;
            IntPtr attributeList = IntPtr.Zero;
            IntPtr inheritedHandles = IntPtr.Zero;
            IntPtr environmentPointer = IntPtr.Zero;
            bool attributeListInitialized = false;
            bool processCreated = false;
            bool assigned = false;
            bool resumed = false;
            PROCESS_INFORMATION processInfo = new PROCESS_INFORMATION();

            try
            {
                executablePin = PinExistingPath(executable, false, FILE_SHARE_READ, "ExecutablePath");
                workingPin = PinExistingPath(working, true, FILE_SHARE_READ | FILE_SHARE_WRITE, "WorkingDirectory");
                outputParentPin = PinExistingPath(outputParent, true, FILE_SHARE_READ | FILE_SHARE_WRITE, "OutputParent");
                AssertPinnedPathUnchanged(executablePin);
                AssertPinnedPathUnchanged(workingPin);
                AssertPinnedPathUnchanged(outputParentPin);

                job = NativeMethods.CreateJobObjectW(IntPtr.Zero, null);
                if (job == null || job.IsInvalid)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateJobObjectW failed.");
                ConfigureKillOnClose(job);

                var inheritable = new SECURITY_ATTRIBUTES
                {
                    nLength = Marshal.SizeOf(typeof(SECURITY_ATTRIBUTES)),
                    lpSecurityDescriptor = IntPtr.Zero,
                    bInheritHandle = true
                };
                stdoutHandle = NativeMethods.CreateFileW(
                    stdout, GENERIC_WRITE, FILE_SHARE_READ, ref inheritable,
                    CREATE_NEW, FILE_ATTRIBUTE_NORMAL, IntPtr.Zero);
                if (stdoutHandle == null || stdoutHandle.IsInvalid)
                {
                    int error = Marshal.GetLastWin32Error();
                    if (error == NativeMethods.ERROR_FILE_EXISTS || error == NativeMethods.ERROR_ALREADY_EXISTS)
                        throw new IsolationInputException("OutputMustBeAbsent");
                    throw new Win32Exception(error, "Creating the standard-output leaf failed.");
                }
                AssertCreatedOutput(stdoutHandle, stdout, outputParentPin);

                stderrHandle = NativeMethods.CreateFileW(
                    stderr, GENERIC_WRITE, FILE_SHARE_READ, ref inheritable,
                    CREATE_NEW, FILE_ATTRIBUTE_NORMAL, IntPtr.Zero);
                if (stderrHandle == null || stderrHandle.IsInvalid)
                {
                    int error = Marshal.GetLastWin32Error();
                    if (error == NativeMethods.ERROR_FILE_EXISTS || error == NativeMethods.ERROR_ALREADY_EXISTS)
                        throw new IsolationInputException("OutputMustBeAbsent");
                    throw new Win32Exception(error, "Creating the standard-error leaf failed.");
                }
                AssertCreatedOutput(stderrHandle, stderr, outputParentPin);

                stdinHandle = NativeMethods.CreateFileW(
                    "NUL", GENERIC_READ, FILE_SHARE_READ, ref inheritable,
                    OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, IntPtr.Zero);
                if (stdinHandle == null || stdinHandle.IsInvalid)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Opening the null standard-input device failed.");

                IntPtr attributeSize = IntPtr.Zero;
                NativeMethods.InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref attributeSize);
                int sizingError = Marshal.GetLastWin32Error();
                if (attributeSize == IntPtr.Zero || (sizingError != NativeMethods.ERROR_INSUFFICIENT_BUFFER && sizingError != 0))
                    throw new Win32Exception(sizingError, "Sizing the process attribute list failed.");

                attributeList = Marshal.AllocHGlobal(attributeSize);
                if (!NativeMethods.InitializeProcThreadAttributeList(attributeList, 1, 0, ref attributeSize))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Initializing the process attribute list failed.");
                attributeListInitialized = true;

                inheritedHandles = Marshal.AllocHGlobal(IntPtr.Size * 3);
                Marshal.WriteIntPtr(inheritedHandles, 0, stdinHandle.DangerousGetHandle());
                Marshal.WriteIntPtr(inheritedHandles, IntPtr.Size, stdoutHandle.DangerousGetHandle());
                Marshal.WriteIntPtr(inheritedHandles, IntPtr.Size * 2, stderrHandle.DangerousGetHandle());
                if (!NativeMethods.UpdateProcThreadAttribute(
                    attributeList, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                    inheritedHandles, new IntPtr(IntPtr.Size * 3), IntPtr.Zero, IntPtr.Zero))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Restricting inherited handles failed.");

                environmentPointer = Marshal.AllocHGlobal(environmentBlock.Length);
                Marshal.Copy(environmentBlock, 0, environmentPointer, environmentBlock.Length);

                var startup = new STARTUPINFOEX();
                startup.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
                startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdInput = stdinHandle.DangerousGetHandle();
                startup.StartupInfo.hStdOutput = stdoutHandle.DangerousGetHandle();
                startup.StartupInfo.hStdError = stderrHandle.DangerousGetHandle();
                startup.lpAttributeList = attributeList;

                AssertPinnedPathUnchanged(executablePin);
                AssertPinnedPathUnchanged(workingPin);
                AssertPinnedPathUnchanged(outputParentPin);
                AssertCreatedOutput(stdoutHandle, stdout, outputParentPin);
                AssertCreatedOutput(stderrHandle, stderr, outputParentPin);
                var mutableCommandLine = new StringBuilder(commandLine, commandLine.Length + 1);
                uint creationFlags = CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT |
                    CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT;
                if (!NativeMethods.CreateProcessW(
                    executable, mutableCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                    creationFlags, environmentPointer, working, ref startup, out processInfo))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateProcessW failed.");

                processCreated = true;
                ThrowIfContractFailure(failurePoint, "RawPostCreate");
                process = new SafeKernelHandle(processInfo.hProcess, true);
                processInfo.hProcess = IntPtr.Zero;
                thread = new SafeKernelHandle(processInfo.hThread, true);
                processInfo.hThread = IntPtr.Zero;
                if (process.IsInvalid || thread.IsInvalid)
                    throw new InvalidOperationException("CreateProcessW returned an invalid process or thread handle.");

                // Assign immediately after safe-handle adoption. All post-create
                // path verification now fails inside the Job cleanup boundary.
                ThrowIfContractFailure(failurePoint, "PreAssign");
                if (!NativeMethods.AssignProcessToJobObject(job, process))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "AssignProcessToJobObject failed.");
                assigned = true;
                ThrowIfContractFailure(failurePoint, "PostAssignPreResume");

                AssertPinnedPathUnchanged(executablePin);
                AssertPinnedPathUnchanged(workingPin);
                AssertPinnedPathUnchanged(outputParentPin);
                AssertCreatedOutput(stdoutHandle, stdout, outputParentPin);
                AssertCreatedOutput(stderrHandle, stderr, outputParentPin);

                ulong creationTime = ReadCreationTime(process);
                ProcessEvidence preResume = QueryEvidence(
                    job, process, checked((int)processInfo.dwProcessId), creationTime);
                if (!preResume.IsProcessInJob || preResume.ActiveProcessCount != 1 ||
                    Array.IndexOf(preResume.ActiveProcessIds, (ulong)processInfo.dwProcessId) < 0)
                    throw new InvalidOperationException("Job membership verification failed before resume.");

                uint previousSuspendCount = NativeMethods.ResumeThread(thread);
                if (previousSuspendCount == UInt32.MaxValue)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "ResumeThread failed.");
                if (previousSuspendCount != 1)
                    throw new InvalidOperationException("The primary thread had an unexpected suspend count.");
                resumed = true;
                thread.Dispose();
                thread = null;
                ThrowIfContractFailure(failurePoint, "PostResume");

                var result = new IsolatedJobProcess(
                    job, process, checked((int)processInfo.dwProcessId), creationTime);
                job = null;
                process = null;
                return result;
            }
            catch (Exception launchError)
            {
                Exception cleanupError = null;
                bool forceRecovery = ForcesContractRecovery(failurePoint);
                if (processCreated && process != null && !process.IsInvalid)
                {
                    Exception terminationRequestError = null;
                    uint waitResult;
                    if (forceRecovery)
                    {
                        terminationRequestError = new InvalidOperationException(
                            "Contract failure injection forced recovery ownership transfer.");
                        waitResult = NativeMethods.WAIT_TIMEOUT;
                    }
                    else
                    {
                        if (assigned && job != null && !job.IsInvalid)
                        {
                            if (!NativeMethods.TerminateJobObject(job, 0xE0010001))
                                terminationRequestError = new Win32Exception(
                                    Marshal.GetLastWin32Error(), "Terminating the failed-launch Job failed.");
                        }
                        else if (!NativeMethods.TerminateProcess(process, 0xE0010002))
                        {
                            terminationRequestError = new Win32Exception(
                                Marshal.GetLastWin32Error(), "Terminating the suspended process failed.");
                        }
                        waitResult = NativeMethods.WaitForSingleObject(process, 5000);
                    }
                    if (waitResult == NativeMethods.WAIT_FAILED)
                        cleanupError = new Win32Exception(
                            Marshal.GetLastWin32Error(), "Waiting for failed-launch cleanup failed.");
                    else if (waitResult != NativeMethods.WAIT_OBJECT_0)
                        cleanupError = new InvalidOperationException(
                            "Failed-launch cleanup could not confirm process termination.");

                    if (waitResult == NativeMethods.WAIT_OBJECT_0 && assigned &&
                        job != null && !job.IsInvalid)
                    {
                        try
                        {
                            int failedProcessId = checked((int)processInfo.dwProcessId);
                            ulong failedCreationTime = ReadCreationTime(process);
                            Stopwatch activeZeroClock = Stopwatch.StartNew();
                            while (true)
                            {
                                ProcessEvidence evidence = QueryEvidence(
                                    job, process, failedProcessId, failedCreationTime);
                                if (evidence.ActiveProcessCount == 0 && evidence.ActiveProcessIds.Length == 0)
                                {
                                    cleanupError = null;
                                    break;
                                }
                                if (activeZeroClock.ElapsedMilliseconds >= 5000)
                                {
                                    cleanupError = new InvalidOperationException(
                                        "Failed-launch Job cleanup did not converge to active-zero.");
                                    break;
                                }
                                Thread.Sleep(25);
                            }
                        }
                        catch (Exception evidenceError)
                        {
                            cleanupError = evidenceError;
                        }
                    }
                    else if (waitResult == NativeMethods.WAIT_OBJECT_0)
                    {
                        cleanupError = null;
                    }

                    if (cleanupError != null && terminationRequestError != null)
                        cleanupError = new AggregateException(
                            "Failed-launch termination request and verification both failed.",
                            terminationRequestError,
                            cleanupError);

                    if (cleanupError != null)
                    {
                        SafeKernelHandle recoveryJob = null;
                        if (assigned && job != null && !job.IsInvalid)
                        {
                            recoveryJob = job;
                            job = null;
                        }
                        var recovery = new FailedLaunchRecoveryProcess(
                            recoveryJob,
                            process,
                            checked((int)processInfo.dwProcessId),
                            TryReadCreationTimeForRecovery(process));
                        process = null;
                        throw new IsolationLaunchCleanupException(
                            launchError, cleanupError, recovery);
                    }
                }
                else if (processCreated && processInfo.hProcess != IntPtr.Zero)
                {
                    Exception terminationRequestError = null;
                    uint waitResult;
                    if (forceRecovery)
                    {
                        terminationRequestError = new InvalidOperationException(
                            "Contract raw-handle failure injection forced recovery ownership transfer.");
                        waitResult = NativeMethods.WAIT_TIMEOUT;
                    }
                    else
                    {
                        if (!NativeMethods.TerminateProcessRaw(processInfo.hProcess, 0xE0010003))
                            terminationRequestError = new Win32Exception(
                                Marshal.GetLastWin32Error(), "Terminating the raw suspended process failed.");
                        waitResult = NativeMethods.WaitForSingleObjectRaw(processInfo.hProcess, 5000);
                    }
                    if (waitResult == NativeMethods.WAIT_FAILED)
                        cleanupError = new Win32Exception(
                            Marshal.GetLastWin32Error(), "Waiting for raw failed-launch cleanup failed.");
                    else if (waitResult != NativeMethods.WAIT_OBJECT_0)
                        cleanupError = new InvalidOperationException(
                            "Raw failed-launch cleanup could not confirm process termination.");
                    if (cleanupError != null && terminationRequestError != null)
                        cleanupError = new AggregateException(
                            "Raw failed-launch termination request and verification both failed.",
                            terminationRequestError,
                            cleanupError);
                    if (cleanupError != null)
                    {
                        var rawProcess = new SafeKernelHandle(processInfo.hProcess, true);
                        processInfo.hProcess = IntPtr.Zero;
                        var recovery = new FailedLaunchRecoveryProcess(
                            null,
                            rawProcess,
                            checked((int)processInfo.dwProcessId),
                            TryReadCreationTimeForRecovery(rawProcess));
                        throw new IsolationLaunchCleanupException(
                            launchError, cleanupError, recovery);
                    }
                }
                if (processInfo.hThread != IntPtr.Zero)
                {
                    if (NativeMethods.CloseHandle(processInfo.hThread))
                        processInfo.hThread = IntPtr.Zero;
                    else
                        cleanupError = new Win32Exception(
                            Marshal.GetLastWin32Error(), "Closing the raw primary-thread handle failed.");
                }
                if (processInfo.hProcess != IntPtr.Zero)
                {
                    if (NativeMethods.CloseHandle(processInfo.hProcess))
                        processInfo.hProcess = IntPtr.Zero;
                    else
                        cleanupError = new Win32Exception(
                            Marshal.GetLastWin32Error(), "Closing the raw process handle failed.");
                }
                if (cleanupError != null)
                    throw new AggregateException(
                        "The isolated launch failed and cleanup could not be confirmed.",
                        launchError, cleanupError);
                throw;
            }
            finally
            {
                if (!resumed && thread != null) thread.Dispose();
                if (process != null) process.Dispose();
                if (processInfo.hThread != IntPtr.Zero) NativeMethods.CloseHandle(processInfo.hThread);
                if (processInfo.hProcess != IntPtr.Zero) NativeMethods.CloseHandle(processInfo.hProcess);
                if (job != null) job.Dispose();
                if (environmentPointer != IntPtr.Zero) Marshal.FreeHGlobal(environmentPointer);
                if (attributeListInitialized) NativeMethods.DeleteProcThreadAttributeList(attributeList);
                if (attributeList != IntPtr.Zero) Marshal.FreeHGlobal(attributeList);
                if (inheritedHandles != IntPtr.Zero) Marshal.FreeHGlobal(inheritedHandles);
                if (stdinHandle != null) stdinHandle.Dispose();
                if (stderrHandle != null) stderrHandle.Dispose();
                if (stdoutHandle != null) stdoutHandle.Dispose();
                if (outputParentPin != null) outputParentPin.Dispose();
                if (workingPin != null) workingPin.Dispose();
                if (executablePin != null) executablePin.Dispose();
            }
        }

        private static void ThrowIfContractFailure(string configuredPoint, string currentPoint)
        {
            if (String.Equals(
                configuredPoint,
                currentPoint + ":Recover",
                StringComparison.Ordinal))
                throw new InvalidOperationException(
                    "Contract failure injection reached " + currentPoint + ".");
        }

        private static bool ForcesContractRecovery(string configuredPoint)
        {
            return configuredPoint != null &&
                configuredPoint.EndsWith(":Recover", StringComparison.Ordinal);
        }

        private static ulong TryReadCreationTimeForRecovery(SafeKernelHandle process)
        {
            try
            {
                return ReadCreationTime(process);
            }
            catch
            {
                // The process SafeHandle itself remains the authoritative pinned
                // identity even if the optional diagnostic FILETIME query fails.
                return 0;
            }
        }

        internal static ProcessEvidence QueryEvidence(
            SafeKernelHandle job,
            SafeKernelHandle process,
            int processId,
            ulong creationFileTimeUtc)
        {
            bool isMember;
            if (!NativeMethods.IsProcessInJob(process, job, out isMember))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "IsProcessInJob failed.");

            for (int attempt = 0; attempt < 16; attempt++)
            {
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION accounting = QueryAccounting(job);
                ulong[] activeIds = QueryActiveProcessIds(job, Math.Max(1U, accounting.ActiveProcesses));
                if (activeIds.Length != accounting.ActiveProcesses)
                    continue;

                return new ProcessEvidence
                {
                    ProcessId = processId,
                    CreationFileTimeUtc = creationFileTimeUtc,
                    IsProcessInJob = isMember,
                    ActiveProcessCount = accounting.ActiveProcesses,
                    TotalProcessCount = accounting.TotalProcesses,
                    TerminatedProcessCount = accounting.TotalTerminatedProcesses,
                    ActiveProcessIds = activeIds
                };
            }
            throw new InvalidOperationException("Job accounting did not stabilize while processes changed.");
        }

        private static JOBOBJECT_BASIC_ACCOUNTING_INFORMATION QueryAccounting(SafeKernelHandle job)
        {
            int accountingLength = Marshal.SizeOf(typeof(JOBOBJECT_BASIC_ACCOUNTING_INFORMATION));
            IntPtr accountingPointer = Marshal.AllocHGlobal(accountingLength);
            try
            {
                uint returned;
                if (!NativeMethods.QueryInformationJobObject(
                    job, JobObjectBasicAccountingInformation, accountingPointer,
                    (uint)accountingLength, out returned))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Querying job accounting failed.");
                return (JOBOBJECT_BASIC_ACCOUNTING_INFORMATION)Marshal.PtrToStructure(
                    accountingPointer, typeof(JOBOBJECT_BASIC_ACCOUNTING_INFORMATION));
            }
            finally
            {
                Marshal.FreeHGlobal(accountingPointer);
            }

        }

        private static ulong[] QueryActiveProcessIds(SafeKernelHandle job, uint initialCapacity)
        {
            uint capacity = Math.Max(4U, initialCapacity);
            for (int attempt = 0; attempt < 8; attempt++)
            {
                int bytes = checked(8 + checked((int)capacity * IntPtr.Size));
                IntPtr buffer = Marshal.AllocHGlobal(bytes);
                try
                {
                    uint returned;
                    if (NativeMethods.QueryInformationJobObject(
                        job, JobObjectBasicProcessIdList, buffer, (uint)bytes, out returned))
                    {
                        uint count = unchecked((uint)Marshal.ReadInt32(buffer, 4));
                        if (count > capacity)
                            throw new InvalidOperationException("The job process identifier list exceeded its buffer.");
                        var result = new ulong[count];
                        for (uint i = 0; i < count; i++)
                        {
                            IntPtr value = Marshal.ReadIntPtr(buffer, checked(8 + (int)i * IntPtr.Size));
                            result[i] = unchecked((ulong)value.ToInt64());
                        }
                        return result;
                    }

                    int error = Marshal.GetLastWin32Error();
                    if (error != NativeMethods.ERROR_MORE_DATA)
                        throw new Win32Exception(error, "Querying job process identifiers failed.");
                    uint assigned = unchecked((uint)Marshal.ReadInt32(buffer, 0));
                    capacity = Math.Max(capacity * 2, Math.Max(assigned, capacity + 1));
                }
                finally
                {
                    Marshal.FreeHGlobal(buffer);
                }
            }
            throw new InvalidOperationException("The job process identifier list did not stabilize.");
        }

        private static void ConfigureKillOnClose(SafeKernelHandle job)
        {
            var information = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            int length = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
            IntPtr pointer = Marshal.AllocHGlobal(length);
            try
            {
                Marshal.StructureToPtr(information, pointer, false);
                if (!NativeMethods.SetInformationJobObject(
                    job, JobObjectExtendedLimitInformation, pointer, (uint)length))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Configuring job limits failed.");
            }
            finally
            {
                Marshal.FreeHGlobal(pointer);
            }
        }

        internal static ulong ReadCreationTime(SafeKernelHandle process)
        {
            FILETIME creation, exit, kernel, user;
            if (!NativeMethods.GetProcessTimes(process, out creation, out exit, out kernel, out user))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "GetProcessTimes failed.");
            return ((ulong)creation.dwHighDateTime << 32) | creation.dwLowDateTime;
        }

        private static string ValidateOutputPathLexical(string value, string parameterName)
        {
            string path = ValidateExactAbsolutePath(value, parameterName);
            string parent = Path.GetDirectoryName(path);
            if (String.IsNullOrEmpty(parent))
                throw new IsolationInputException("OutputParentMissing");
            string leaf = Path.GetFileName(path);
            if (String.IsNullOrWhiteSpace(leaf) || leaf == "." || leaf == ".." ||
                leaf.EndsWith(" ", StringComparison.Ordinal) || leaf.EndsWith(".", StringComparison.Ordinal) ||
                leaf.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0 || IsDosDeviceName(leaf))
                throw new IsolationInputException("OutputLeafUnsafe");
            if (File.Exists(path) || Directory.Exists(path))
                throw new IsolationInputException("OutputMustBeAbsent");
            return path;
        }

        private static string ValidateExactAbsolutePath(string value, string parameterName)
        {
            if (String.IsNullOrWhiteSpace(value) || value.IndexOf('\0') >= 0)
                throw new IsolationInputException("PathRequired");
            if (!Path.IsPathFullyQualified(value))
                throw new IsolationInputException("PathMustBeAbsolute");
            if (value.Length < 3 || !Char.IsLetter(value[0]) || value[1] != ':' || value[2] != '\\')
                throw new IsolationInputException("PathNamespaceUnsupported");
            string full = Path.GetFullPath(value);
            if (!String.Equals(full, value, StringComparison.OrdinalIgnoreCase))
                throw new IsolationInputException("PathMustBeCanonical");
            try
            {
                var drive = new DriveInfo(Path.GetPathRoot(full));
                if (drive.DriveType != DriveType.Fixed)
                    throw new IsolationInputException("PathVolumeUnsupported");
            }
            catch (IsolationInputException) { throw; }
            catch { throw new IsolationInputException("PathVolumeUnsupported"); }
            return full;
        }

        private static PinnedPath PinExistingPath(
            string expectedPath,
            bool expectDirectory,
            uint shareMode,
            string parameterName)
        {
            uint flags = FILE_FLAG_OPEN_REPARSE_POINT;
            if (expectDirectory) flags |= FILE_FLAG_BACKUP_SEMANTICS;
            SafeFileHandle handle = NativeMethods.OpenPathHandleW(
                expectedPath, FILE_READ_ATTRIBUTES, shareMode, IntPtr.Zero,
                OPEN_EXISTING, flags, IntPtr.Zero);
            if (handle == null || handle.IsInvalid)
            {
                if (handle != null) handle.Dispose();
                throw new IsolationInputException("PathOpenFailed");
            }

            try
            {
                FILE_ATTRIBUTE_TAG_INFO attributes = ReadAttributeTag(handle);
                bool isDirectory = (((FileAttributes)attributes.FileAttributes) & FileAttributes.Directory) != 0;
                bool isReparse = (((FileAttributes)attributes.FileAttributes) & FileAttributes.ReparsePoint) != 0;
                if (isReparse) throw new IsolationInputException("PathReparse");
                if (isDirectory != expectDirectory) throw new IsolationInputException("PathKindMismatch");
                string finalPath = ReadFinalDosPath(handle);
                if (!String.Equals(finalPath, expectedPath, StringComparison.OrdinalIgnoreCase))
                    throw new IsolationInputException("PathFinalMismatch");
                string identity = ReadFileIdentity(handle);
                return new PinnedPath(handle, expectedPath, finalPath, identity, isDirectory);
            }
            catch
            {
                handle.Dispose();
                throw;
            }
        }

        private static void AssertPinnedPathUnchanged(PinnedPath pinned)
        {
            if (pinned == null || pinned.Handle == null || pinned.Handle.IsClosed || pinned.Handle.IsInvalid)
                throw new InvalidOperationException("A required pinned path handle is unavailable.");
            FILE_ATTRIBUTE_TAG_INFO attributes = ReadAttributeTag(pinned.Handle);
            bool isDirectory = (((FileAttributes)attributes.FileAttributes) & FileAttributes.Directory) != 0;
            bool isReparse = (((FileAttributes)attributes.FileAttributes) & FileAttributes.ReparsePoint) != 0;
            if (isReparse || isDirectory != pinned.IsDirectory ||
                !String.Equals(ReadFinalDosPath(pinned.Handle), pinned.FinalPath, StringComparison.OrdinalIgnoreCase) ||
                !String.Equals(ReadFileIdentity(pinned.Handle), pinned.Identity, StringComparison.Ordinal))
                throw new InvalidOperationException("A pinned path identity changed during launch.");
        }

        private static void AssertCreatedOutput(
            SafeFileHandle output,
            string expectedPath,
            PinnedPath parent)
        {
            if (output == null || output.IsClosed || output.IsInvalid)
                throw new InvalidOperationException("A created output handle is unavailable.");
            FILE_ATTRIBUTE_TAG_INFO attributes = ReadAttributeTag(output);
            FileAttributes fileAttributes = (FileAttributes)attributes.FileAttributes;
            if ((fileAttributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                throw new InvalidOperationException("A created output is not a regular file.");
            string finalPath = ReadFinalDosPath(output);
            if (!String.Equals(finalPath, expectedPath, StringComparison.OrdinalIgnoreCase) ||
                !String.Equals(Path.GetDirectoryName(finalPath), parent.FinalPath, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException("A created output escaped its pinned parent.");
            if (String.IsNullOrEmpty(ReadFileIdentity(output)))
                throw new InvalidOperationException("A created output has no native file identity.");
        }

        private static FILE_ATTRIBUTE_TAG_INFO ReadAttributeTag(SafeFileHandle handle)
        {
            FILE_ATTRIBUTE_TAG_INFO information;
            if (!NativeMethods.GetFileInformationByHandleEx(
                handle, 9, out information, (uint)Marshal.SizeOf(typeof(FILE_ATTRIBUTE_TAG_INFO))))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Reading path attributes failed.");
            return information;
        }

        private static string ReadFileIdentity(SafeFileHandle handle)
        {
            FILE_ID_INFO information;
            if (!NativeMethods.GetFileInformationByHandleEx(
                handle, 18, out information, (uint)Marshal.SizeOf(typeof(FILE_ID_INFO))))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Reading native path identity failed.");
            if (information.FileId == null || information.FileId.Length != 16)
                throw new InvalidOperationException("Native path identity is malformed.");
            return information.VolumeSerialNumber.ToString("x16") + ":" +
                BitConverter.ToString(information.FileId).Replace("-", "").ToLowerInvariant();
        }

        private static string ReadFinalDosPath(SafeFileHandle handle)
        {
            uint capacity = 512;
            for (int attempt = 0; attempt < 8; attempt++)
            {
                var buffer = new StringBuilder(checked((int)capacity));
                uint length = NativeMethods.GetFinalPathNameByHandleW(handle, buffer, capacity, 0);
                if (length == 0)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Reading final path failed.");
                if (length >= capacity)
                {
                    capacity = checked(length + 1);
                    continue;
                }
                string nativePath = buffer.ToString();
                if (!nativePath.StartsWith(@"\\?\", StringComparison.Ordinal) ||
                    nativePath.Length < 7 || !Char.IsLetter(nativePath[4]) ||
                    nativePath[5] != ':' || nativePath[6] != '\\')
                    throw new IsolationInputException("PathFinalNamespaceUnsupported");
                string dosPath = nativePath.Substring(4);
                if (!String.Equals(Path.GetFullPath(dosPath), dosPath, StringComparison.OrdinalIgnoreCase))
                    throw new IsolationInputException("PathFinalNonCanonical");
                return dosPath;
            }
            throw new InvalidOperationException("Final path length did not stabilize.");
        }

        private static bool IsDosDeviceName(string leaf)
        {
            string stem = leaf.Split('.')[0].TrimEnd(' ');
            if (stem.Equals("CON", StringComparison.OrdinalIgnoreCase) ||
                stem.Equals("PRN", StringComparison.OrdinalIgnoreCase) ||
                stem.Equals("AUX", StringComparison.OrdinalIgnoreCase) ||
                stem.Equals("NUL", StringComparison.OrdinalIgnoreCase)) return true;
            if (stem.Length == 4 && (stem.StartsWith("COM", StringComparison.OrdinalIgnoreCase) ||
                stem.StartsWith("LPT", StringComparison.OrdinalIgnoreCase)) &&
                stem[3] >= '1' && stem[3] <= '9') return true;
            return false;
        }

        private static string BuildCommandLine(string executable, IReadOnlyList<string> arguments)
        {
            if (arguments == null) throw new IsolationInputException("ArgumentListMissing");
            var builder = new StringBuilder();
            builder.Append(QuoteArgument(executable));
            for (int i = 0; i < arguments.Count; i++)
            {
                string argument = arguments[i];
                if (argument == null)
                    throw new IsolationInputException("ArgumentNull");
                for (int c = 0; c < argument.Length; c++)
                {
                    if (Char.IsControl(argument[c]))
                        throw new IsolationInputException("ArgumentControlCharacter");
                }
                builder.Append(' ');
                builder.Append(QuoteArgument(argument));
            }
            if (builder.Length > 32766)
                throw new IsolationInputException("CommandLineTooLong");
            return builder.ToString();
        }

        private static string QuoteArgument(string value)
        {
            if (value.Length > 0 && value.IndexOfAny(new[] { ' ', '\t', '\v', '\"' }) < 0)
                return value;

            var result = new StringBuilder();
            result.Append('\"');
            int slashes = 0;
            foreach (char ch in value)
            {
                if (ch == '\\')
                {
                    slashes++;
                    continue;
                }
                if (ch == '\"')
                {
                    result.Append('\\', slashes * 2 + 1);
                    result.Append('\"');
                    slashes = 0;
                    continue;
                }
                result.Append('\\', slashes);
                slashes = 0;
                result.Append(ch);
            }
            result.Append('\\', slashes * 2);
            result.Append('\"');
            return result.ToString();
        }

        private static byte[] BuildEnvironmentBlock(IReadOnlyDictionary<string, string> environment)
        {
            if (environment == null) throw new IsolationInputException("EnvironmentMissing");
            var sorted = new SortedDictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            foreach (KeyValuePair<string, string> pair in environment)
            {
                if (pair.Key == null || !EnvironmentKey.IsMatch(pair.Key))
                    throw new IsolationInputException("EnvironmentKey");
                if (pair.Value == null || pair.Value.IndexOf('\0') >= 0)
                    throw new IsolationInputException("EnvironmentValue");
                try
                {
                    sorted.Add(pair.Key, pair.Value);
                }
                catch (ArgumentException)
                {
                    throw new IsolationInputException("EnvironmentDuplicateKey");
                }
            }

            var builder = new StringBuilder();
            foreach (KeyValuePair<string, string> pair in sorted)
            {
                builder.Append(pair.Key);
                builder.Append('=');
                builder.Append(pair.Value);
                builder.Append('\0');
            }
            builder.Append('\0');
            if (sorted.Count == 0) builder.Append('\0');
            if (builder.Length > 32767)
                throw new IsolationInputException("EnvironmentBlockTooLong");
            return Encoding.Unicode.GetBytes(builder.ToString());
        }

    }

    internal static class NativeMethods
    {
        internal const uint WAIT_OBJECT_0 = 0;
        internal const uint WAIT_TIMEOUT = 258;
        internal const uint WAIT_FAILED = 0xFFFFFFFF;
        internal const int ERROR_FILE_EXISTS = 80;
        internal const int ERROR_INSUFFICIENT_BUFFER = 122;
        internal const int ERROR_ALREADY_EXISTS = 183;
        internal const int ERROR_MORE_DATA = 234;

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool CloseHandle(IntPtr handle);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern SafeKernelHandle CreateJobObjectW(IntPtr attributes, string name);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool SetInformationJobObject(
            SafeKernelHandle job, int informationClass, IntPtr information, uint informationLength);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool QueryInformationJobObject(
            SafeKernelHandle job, int informationClass, IntPtr information,
            uint informationLength, out uint returnLength);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool AssignProcessToJobObject(SafeKernelHandle job, SafeKernelHandle process);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool IsProcessInJob(
            SafeKernelHandle process, SafeKernelHandle job, [MarshalAs(UnmanagedType.Bool)] out bool result);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool TerminateJobObject(SafeKernelHandle job, uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool TerminateProcess(SafeKernelHandle process, uint exitCode);

        [DllImport("kernel32.dll", EntryPoint = "TerminateProcess", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool TerminateProcessRaw(IntPtr process, uint exitCode);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool CreateProcessW(
            string applicationName, StringBuilder commandLine,
            IntPtr processAttributes, IntPtr threadAttributes,
            [MarshalAs(UnmanagedType.Bool)] bool inheritHandles, uint creationFlags,
            IntPtr environment, string currentDirectory,
            ref STARTUPINFOEX startupInfo, out PROCESS_INFORMATION processInformation);

        [DllImport("kernel32.dll", SetLastError = true)]
        internal static extern uint ResumeThread(SafeKernelHandle thread);

        [DllImport("kernel32.dll", SetLastError = true)]
        internal static extern uint WaitForSingleObject(SafeKernelHandle handle, uint milliseconds);

        [DllImport("kernel32.dll", EntryPoint = "WaitForSingleObject", SetLastError = true)]
        internal static extern uint WaitForSingleObjectRaw(IntPtr handle, uint milliseconds);

        [DllImport("kernel32.dll", EntryPoint = "GetExitCodeProcess", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetExitCodeProcessRaw(IntPtr process, out uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetProcessTimes(
            SafeKernelHandle process, out FILETIME creation, out FILETIME exit,
            out FILETIME kernel, out FILETIME user);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool InitializeProcThreadAttributeList(
            IntPtr attributeList, int attributeCount, int flags, ref IntPtr size);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool UpdateProcThreadAttribute(
            IntPtr attributeList, uint flags, IntPtr attribute, IntPtr value,
            IntPtr size, IntPtr previousValue, IntPtr returnSize);

        [DllImport("kernel32.dll")]
        internal static extern void DeleteProcThreadAttributeList(IntPtr attributeList);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern SafeFileHandle CreateFileW(
            string fileName, uint desiredAccess, uint shareMode,
            ref SECURITY_ATTRIBUTES securityAttributes, uint creationDisposition,
            uint flagsAndAttributes, IntPtr templateFile);

        [DllImport("kernel32.dll", EntryPoint = "CreateFileW", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern SafeFileHandle OpenPathHandleW(
            string fileName, uint desiredAccess, uint shareMode,
            IntPtr securityAttributes, uint creationDisposition,
            uint flagsAndAttributes, IntPtr templateFile);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern uint GetFinalPathNameByHandleW(
            SafeFileHandle handle, StringBuilder path, uint pathLength, uint flags);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetFileInformationByHandleEx(
            SafeFileHandle handle, int informationClass,
            out FILE_ATTRIBUTE_TAG_INFO information, uint informationLength);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetFileInformationByHandleEx(
            SafeFileHandle handle, int informationClass,
            out FILE_ID_INFO information, uint informationLength);
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct SECURITY_ATTRIBUTES
    {
        internal int nLength;
        internal IntPtr lpSecurityDescriptor;
        [MarshalAs(UnmanagedType.Bool)] internal bool bInheritHandle;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct FILE_ATTRIBUTE_TAG_INFO
    {
        internal uint FileAttributes;
        internal uint ReparseTag;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct FILE_ID_INFO
    {
        internal ulong VolumeSerialNumber;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)]
        internal byte[] FileId;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    internal struct STARTUPINFO
    {
        internal int cb;
        internal string lpReserved;
        internal string lpDesktop;
        internal string lpTitle;
        internal uint dwX;
        internal uint dwY;
        internal uint dwXSize;
        internal uint dwYSize;
        internal uint dwXCountChars;
        internal uint dwYCountChars;
        internal uint dwFillAttribute;
        internal uint dwFlags;
        internal ushort wShowWindow;
        internal ushort cbReserved2;
        internal IntPtr lpReserved2;
        internal IntPtr hStdInput;
        internal IntPtr hStdOutput;
        internal IntPtr hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct STARTUPINFOEX
    {
        internal STARTUPINFO StartupInfo;
        internal IntPtr lpAttributeList;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct PROCESS_INFORMATION
    {
        internal IntPtr hProcess;
        internal IntPtr hThread;
        internal uint dwProcessId;
        internal uint dwThreadId;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct FILETIME
    {
        internal uint dwLowDateTime;
        internal uint dwHighDateTime;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct JOBOBJECT_BASIC_LIMIT_INFORMATION
    {
        internal long PerProcessUserTimeLimit;
        internal long PerJobUserTimeLimit;
        internal uint LimitFlags;
        internal UIntPtr MinimumWorkingSetSize;
        internal UIntPtr MaximumWorkingSetSize;
        internal uint ActiveProcessLimit;
        internal UIntPtr Affinity;
        internal uint PriorityClass;
        internal uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct IO_COUNTERS
    {
        internal ulong ReadOperationCount;
        internal ulong WriteOperationCount;
        internal ulong OtherOperationCount;
        internal ulong ReadTransferCount;
        internal ulong WriteTransferCount;
        internal ulong OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    {
        internal JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        internal IO_COUNTERS IoInfo;
        internal UIntPtr ProcessMemoryLimit;
        internal UIntPtr JobMemoryLimit;
        internal UIntPtr PeakProcessMemoryUsed;
        internal UIntPtr PeakJobMemoryUsed;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct JOBOBJECT_BASIC_ACCOUNTING_INFORMATION
    {
        internal long TotalUserTime;
        internal long TotalKernelTime;
        internal long ThisPeriodTotalUserTime;
        internal long ThisPeriodTotalKernelTime;
        internal uint TotalPageFaultCount;
        internal uint TotalProcesses;
        internal uint ActiveProcesses;
        internal uint TotalTerminatedProcesses;
    }
}
'@
}

if ([Dynamo.Perf.Isolation.NativeLauncher]::SourceVersion -cne $script:ExpectedNativeSourceVersion) {
    throw 'isolated-process-job: a stale native type is already loaded in this PowerShell process'
}

function Start-DynamoIsolatedProcess {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$ExecutablePath,
        [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$ArgumentList,
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [Parameter(Mandatory)][object]$Environment,
        [Parameter(Mandatory)][string]$StandardOutputPath,
        [Parameter(Mandatory)][string]$StandardErrorPath
    )

    if ($Environment -isnot [System.Collections.IDictionary]) {
        throw [Dynamo.Perf.Isolation.IsolationInputException]::new('EnvironmentType')
    }

    $arguments = [System.Collections.Generic.List[string]]::new()
    foreach ($argument in $ArgumentList) {
        if ($argument -isnot [string]) {
            throw [Dynamo.Perf.Isolation.IsolationInputException]::new('ArgumentType')
        }
        $arguments.Add($argument)
    }

    $environmentCopy = [System.Collections.Generic.Dictionary[string,string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $Environment.GetEnumerator()) {
        if ($entry.Key -isnot [string] -or $entry.Value -isnot [string]) {
            throw [Dynamo.Perf.Isolation.IsolationInputException]::new('EnvironmentType')
        }
        if (-not $environmentCopy.TryAdd($entry.Key, $entry.Value)) {
            throw [Dynamo.Perf.Isolation.IsolationInputException]::new('EnvironmentDuplicateKey')
        }
    }

    $process = [Dynamo.Perf.Isolation.NativeLauncher]::Start(
        $ExecutablePath,
        $arguments,
        $WorkingDirectory,
        $environmentCopy,
        $StandardOutputPath,
        $StandardErrorPath)
    $process
}

function Get-DynamoIsolatedProcessEvidence {
    [CmdletBinding()]
    param([Parameter(Mandatory)][object]$Process)

    if ($Process -isnot [Dynamo.Perf.Isolation.IIsolationRecoveryProcess]) {
        throw 'isolated-process-job: Process is not an isolated or failed-launch recovery handle'
    }
    if (-not $Process.IsJobOpen) {
        throw 'isolated-process-job: failed launch was not assigned to a Job Object'
    }
    $Process.GetEvidence()
}

function Wait-DynamoIsolatedProcess {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object]$Process,
        [Parameter(Mandatory)][ValidateRange(0, 2147483647)][int]$TimeoutMilliseconds
    )

    if ($Process -isnot [Dynamo.Perf.Isolation.IIsolationRecoveryProcess]) {
        throw 'isolated-process-job: Process is not an isolated or failed-launch recovery handle'
    }
    $Process.Wait($TimeoutMilliseconds)
}

function Stop-DynamoIsolatedProcess {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object]$Process,
        [uint32]$ExitCode = 3758161936
    )

    if ($Process -isnot [Dynamo.Perf.Isolation.IIsolationRecoveryProcess]) {
        throw 'isolated-process-job: Process is not an isolated or failed-launch recovery handle'
    }
    $Process.Terminate($ExitCode)
}

function Remove-DynamoIsolatedProcess {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object]$Process,
        [ValidateRange(1, 60000)][int]$TimeoutMilliseconds = 10000
    )

    if ($Process -isnot [Dynamo.Perf.Isolation.IIsolationRecoveryProcess]) {
        throw 'isolated-process-job: Process is not an isolated or failed-launch recovery handle'
    }
    $clock = [Diagnostics.Stopwatch]::StartNew()
    Stop-DynamoIsolatedProcess -Process $Process
    if ($Process.IsJobOpen) {
        while ($true) {
            $evidence = Get-DynamoIsolatedProcessEvidence -Process $Process
            if ($evidence.ActiveProcessCount -eq 0 -and $evidence.ActiveProcessIds.Count -eq 0) {
                break
            }
            $remaining = $TimeoutMilliseconds - $clock.ElapsedMilliseconds
            if ($remaining -le 0) {
                throw 'isolated-process-job: Job active-zero verification timed out; handles were preserved'
            }
            Start-Sleep -Milliseconds ([int][Math]::Min(25, [Math]::Ceiling($remaining)))
        }
    }
    $remaining = [int][Math]::Max(0, [Math]::Ceiling($TimeoutMilliseconds - $clock.ElapsedMilliseconds))
    $waitResult = Wait-DynamoIsolatedProcess -Process $Process -TimeoutMilliseconds $remaining
    if (-not $waitResult.Exited) {
        throw 'isolated-process-job: parent termination verification timed out; handles were preserved'
    }
    $Process.VerifyTerminatedIdentity()
    if ($Process.IsJobOpen) {
        $finalEvidence = Get-DynamoIsolatedProcessEvidence -Process $Process
        if ($finalEvidence.ActiveProcessCount -ne 0 -or $finalEvidence.ActiveProcessIds.Count -ne 0) {
            throw 'isolated-process-job: Job active-zero evidence changed before disposal; handles were preserved'
        }
    }
    $Process.Dispose()
}

Export-ModuleMember -Function @(
    'Start-DynamoIsolatedProcess'
    'Get-DynamoIsolatedProcessEvidence'
    'Wait-DynamoIsolatedProcess'
    'Stop-DynamoIsolatedProcess'
    'Remove-DynamoIsolatedProcess'
)

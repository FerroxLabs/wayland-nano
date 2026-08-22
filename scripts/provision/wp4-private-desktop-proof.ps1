$ErrorActionPreference = 'Stop'
$accountName = 'wngc' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
$random = [byte[]]::new(32)
[Security.Cryptography.RandomNumberGenerator]::Fill($random)
$accountPassword = 'Aa1!' + [Convert]::ToHexString($random)
$computer = [ADSI]'WinNT://.'
$accountCreated = $false

$source = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;

public static class Wp4PrivateDesktopProof {
    const UInt32 PRIVATE_OBJECT_FULL_ACCESS = 0x000F03FF;
    const UInt32 CREATE_SUSPENDED = 0x00000004;
    const UInt32 CREATE_UNICODE_ENVIRONMENT = 0x00000400;
    const UInt32 CREATE_NO_WINDOW = 0x08000000;
    const UInt32 WAIT_OBJECT_0 = 0x00000000;
    const UInt32 WAIT_TIMEOUT = 0x00000102;

    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    struct STARTUPINFO { public UInt32 cb; public string lpReserved; public string lpDesktop; public string lpTitle; public UInt32 dwX; public UInt32 dwY; public UInt32 dwXSize; public UInt32 dwYSize; public UInt32 dwXCountChars; public UInt32 dwYCountChars; public UInt32 dwFillAttribute; public UInt32 dwFlags; public Int16 wShowWindow; public Int16 cbReserved2; public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess; public IntPtr hThread; public UInt32 dwProcessId; public UInt32 dwThreadId; }

    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool CreateProcessWithLogonW(string username,string domain,string password,UInt32 logonFlags,string application,StringBuilder command,UInt32 creationFlags,IntPtr environment,string currentDirectory,ref STARTUPINFO startup,out PROCESS_INFORMATION process);
    [DllImport("user32.dll", SetLastError=true)] static extern IntPtr GetProcessWindowStation();
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateWindowStationW(string name,UInt32 flags,UInt32 access,IntPtr securityAttributes);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetProcessWindowStation(IntPtr station);
    [DllImport("user32.dll", SetLastError=true)] static extern bool CloseWindowStation(IntPtr station);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateDesktopW(string name,string device,IntPtr deviceMode,UInt32 flags,UInt32 access,IntPtr securityAttributes);
    [DllImport("user32.dll", SetLastError=true)] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetUserObjectSecurity(IntPtr handle,ref UInt32 requested,byte[] security,UInt32 length,out UInt32 needed);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetUserObjectSecurity(IntPtr handle,ref UInt32 requested,byte[] security);
    [DllImport("kernel32.dll", SetLastError=true)] static extern UInt32 ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError=true)] static extern UInt32 WaitForSingleObject(IntPtr handle,UInt32 milliseconds);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr process,out UInt32 exitCode);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateProcess(IntPtr process,UInt32 exitCode);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);

    static void Check(bool ok,string operation) { if(!ok)throw new Win32Exception(Marshal.GetLastWin32Error(),operation); }
    static byte[] Security(IntPtr handle) { UInt32 requested=7,needed;GetUserObjectSecurity(handle,ref requested,null,0,out needed);if(needed==0)throw new Win32Exception(Marshal.GetLastWin32Error(),"GetUserObjectSecurity(size)");byte[] bytes=new byte[needed];Check(GetUserObjectSecurity(handle,ref requested,bytes,needed,out needed),"GetUserObjectSecurity");return bytes; }
    static void GrantAccess(IntPtr handle,SecurityIdentifier sid,UInt32 access,string kind,UInt32 canonicalMask) { var descriptor=new RawSecurityDescriptor(Security(handle),0);if(descriptor.DiscretionaryAcl==null)descriptor.DiscretionaryAcl=new RawAcl(2,1);descriptor.DiscretionaryAcl.InsertAce(descriptor.DiscretionaryAcl.Count,new CommonAce(AceFlags.None,AceQualifier.AccessAllowed,unchecked((int)access),sid,false,null));byte[] bytes=new byte[descriptor.BinaryLength];descriptor.GetBinaryForm(bytes,0);UInt32 requested=4;Check(SetUserObjectSecurity(handle,ref requested,bytes),"SetUserObjectSecurity("+kind+")");var applied=new RawSecurityDescriptor(Security(handle),0);bool found=false;if(applied.DiscretionaryAcl!=null)foreach(GenericAce ace in applied.DiscretionaryAcl){var known=ace as KnownAce;if(known!=null&&known.SecurityIdentifier.Equals(sid)&&(unchecked((UInt32)known.AccessMask)&canonicalMask)==canonicalMask)found=true;}if(!found)throw new InvalidOperationException(kind+" access mismatch"); }

    public static string[] Run(string accountName,string accountPassword,byte[] accountSid) {
        IntPtr brokerStation=IntPtr.Zero,privateStation=IntPtr.Zero,privateDesktop=IntPtr.Zero;
        bool privateSelected=false;
        try {
            brokerStation=GetProcessWindowStation();Check(brokerStation!=IntPtr.Zero,"GetProcessWindowStation");
            string stationName="NanoSandboxGateProof-"+Guid.NewGuid().ToString("N"),desktopName="NanoSandboxGateProofDesktop";
            var expected=new SecurityIdentifier(accountSid,0);
            privateStation=CreateWindowStationW(stationName,0,PRIVATE_OBJECT_FULL_ACCESS,IntPtr.Zero);Check(privateStation!=IntPtr.Zero,"CreateWindowStationW");
            GrantAccess(privateStation,expected,PRIVATE_OBJECT_FULL_ACCESS,"window station",0x000F037F); // OS retains defined station rights only
            Check(SetProcessWindowStation(privateStation),"SetProcessWindowStation(private)");privateSelected=true;
            try { privateDesktop=CreateDesktopW(desktopName,null,IntPtr.Zero,0,PRIVATE_OBJECT_FULL_ACCESS,IntPtr.Zero);Check(privateDesktop!=IntPtr.Zero,"CreateDesktopW"); }
            finally { Check(SetProcessWindowStation(brokerStation),"SetProcessWindowStation(broker)");privateSelected=false; }
            GrantAccess(privateDesktop,expected,PRIVATE_OBJECT_FULL_ACCESS,"desktop",0x000F01FF); // OS retains defined desktop rights only
            var results=new System.Collections.Generic.List<string>();results.Add("private_objects_access=ephemeral_account_full");
            foreach(string probe in new[]{"git.exe --version >nul","node.exe --version >nul"}) {
                for(int iteration=1;iteration<=5;iteration++) {
                    var startup=new STARTUPINFO();startup.cb=(UInt32)Marshal.SizeOf(startup);startup.lpDesktop=stationName+"\\"+desktopName;
                    PROCESS_INFORMATION process=new PROCESS_INFORMATION();var timer=System.Diagnostics.Stopwatch.StartNew();
                    string command="\""+Environment.GetEnvironmentVariable("COMSPEC")+"\" /d /s /c \""+probe+"\"";
                    Check(CreateProcessWithLogonW(accountName,".",accountPassword,0,null,new StringBuilder(command),CREATE_SUSPENDED|CREATE_NO_WINDOW|CREATE_UNICODE_ENVIRONMENT,IntPtr.Zero,null,ref startup,out process),"CreateProcessWithLogonW");
                    try {
                        if(ResumeThread(process.hThread)==UInt32.MaxValue)throw new Win32Exception(Marshal.GetLastWin32Error(),"ResumeThread");
                        UInt32 wait=WaitForSingleObject(process.hProcess,30000);if(wait==WAIT_TIMEOUT){TerminateProcess(process.hProcess,124);WaitForSingleObject(process.hProcess,30000);throw new TimeoutException(probe+" timed out");}if(wait!=WAIT_OBJECT_0)throw new Win32Exception(Marshal.GetLastWin32Error(),"WaitForSingleObject");
                        UInt32 exitCode;Check(GetExitCodeProcess(process.hProcess,out exitCode),"GetExitCodeProcess");timer.Stop();if(exitCode!=0)throw new InvalidOperationException(probe+" exit "+exitCode);results.Add("probe="+probe+" iteration="+iteration+" exit="+exitCode+" elapsed_ms="+timer.ElapsedMilliseconds);
                    } finally { if(process.hThread!=IntPtr.Zero)CloseHandle(process.hThread);if(process.hProcess!=IntPtr.Zero)CloseHandle(process.hProcess); }
                }
            }
            return results.ToArray();
        } finally {
            if(privateSelected&&brokerStation!=IntPtr.Zero)SetProcessWindowStation(brokerStation);
            if(privateDesktop!=IntPtr.Zero)CloseDesktop(privateDesktop);
            if(privateStation!=IntPtr.Zero)CloseWindowStation(privateStation);
        }
    }
}
'@

Add-Type -TypeDefinition $source
try {
  $account = $computer.Create('User', $accountName)
  $account.SetPassword($accountPassword)
  $account.SetInfo()
  $accountCreated = $true
  $account = [ADSI]"WinNT://./$accountName,user"
  $sidValue = [Security.Principal.SecurityIdentifier]::new([byte[]]$account.objectSid.Value, 0)
  $sidBytes = [byte[]]::new($sidValue.BinaryLength)
  $sidValue.GetBinaryForm($sidBytes, 0)
  [Wp4PrivateDesktopProof]::Run($accountName, $accountPassword, $sidBytes)
} finally {
  if ($accountCreated) { $computer.Delete('User', $accountName) }
}

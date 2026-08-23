# ConPty TUI driver: launches wayland-nano's interactive TUI on a pseudo-console,
# types a prompt like a human, reads the rendered screen, exits the app.
# Credential: FLUX_API_KEY_FILE (path only) set by the caller.
param(
  [string]$Bin = 'F:\CargoTarget\wayland-nano\release\nano-tui.exe',
  [string]$Root = 'F:\tmp\human-harness-tui'
)
$ErrorActionPreference = 'Stop'

$source = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class ConPtyDrive {
    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential)] struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    struct STARTUPINFO { public int cb; public string lpReserved; public string lpDesktop; public string lpTitle; public int dwX; public int dwY; public int dwXSize; public int dwYSize; public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute; public int dwFlags; public short wShowWindow; public short cbReserved2; public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError; }
    [StructLayout(LayoutKind.Sequential)] struct PROCESS_INFORMATION { public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId; }
    [StructLayout(LayoutKind.Sequential)] struct SECURITY_ATTRIBUTES { public int nLength; public IntPtr lpSecurityDescriptor; public bool bInheritHandle; }

    [DllImport("kernel32.dll", SetLastError=true)] static extern bool CreatePipe(out IntPtr readPipe, out IntPtr writePipe, ref SECURITY_ATTRIBUTES attrs, int size);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern int CreatePseudoConsole(COORD size, IntPtr inputRead, IntPtr outputWrite, int flags, out IntPtr hPC);
    [DllImport("kernel32.dll", SetLastError=true)] static extern void ClosePseudoConsole(IntPtr hPC);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, int flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool UpdateProcThreadAttribute(IntPtr list, int flags, IntPtr attribute, IntPtr value, IntPtr size, IntPtr prev, IntPtr ret);
    [DllImport("kernel32.dll", SetLastError=true)] static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, int flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h, int ms);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr h, uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool ReadFile(IntPtr h, byte[] buf, int bytes, out int read, IntPtr overlapped);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool PeekNamedPipe(IntPtr h, IntPtr buf, int size, IntPtr read, out int avail, IntPtr left);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool WriteFile(IntPtr h, byte[] buf, int bytes, out int written, IntPtr overlapped);
    [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr h, out uint code);

    const int EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    const int CREATE_UNICODE_ENVIRONMENT = 0x00000400;
    static readonly IntPtr PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = (IntPtr)0x20016;

    static void Check(bool ok, string op) { if (!ok) throw new Win32Exception(Marshal.GetLastWin32Error(), op); }

    public static string Drive(string binary, string cwd, string prompt, string expect, string quit, int settleMs, int timeoutMs) {
        var sa = new SECURITY_ATTRIBUTES { nLength = Marshal.SizeOf<SECURITY_ATTRIBUTES>(), bInheritHandle = true };
        IntPtr inRead, inWrite, outRead, outWrite;
        Check(CreatePipe(out inRead, out inWrite, ref sa, 0), "CreatePipe(in)");
        Check(CreatePipe(out outRead, out outWrite, ref sa, 0), "CreatePipe(out)");
        IntPtr hPC;
        Check(CreatePseudoConsole(new COORD { X = 120, Y = 40 }, inRead, outWrite, 0, out hPC) == 0, "CreatePseudoConsole");
        CloseHandle(inRead); CloseHandle(outWrite);

        IntPtr listSize = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref listSize);
        IntPtr attrList = Marshal.AllocHGlobal(listSize);
        Check(InitializeProcThreadAttributeList(attrList, 1, 0, ref listSize), "InitializeProcThreadAttributeList");
        Check(UpdateProcThreadAttribute(attrList, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hPC, (IntPtr)Marshal.SizeOf<IntPtr>(), IntPtr.Zero, IntPtr.Zero), "UpdateProcThreadAttribute");

        var si = new STARTUPINFOEX(); si.StartupInfo.cb = Marshal.SizeOf<STARTUPINFOEX>(); si.lpAttributeList = attrList;
        var pi = new PROCESS_INFORMATION();
        var screen = new StringBuilder();
        try {
            Check(CreateProcessW(null, new StringBuilder("\"" + binary + "\""), IntPtr.Zero, IntPtr.Zero, false, EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT, IntPtr.Zero, cwd, ref si, out pi), "CreateProcessW");
            var buf = new byte[65536];
            Func<int, string> drain = (ms) => {
                var until = DateTime.UtcNow.AddMilliseconds(ms);
                while (DateTime.UtcNow < until) {
                    int avail = 0;
                    if (!PeekNamedPipe(outRead, IntPtr.Zero, 0, IntPtr.Zero, out avail, IntPtr.Zero) || avail == 0) { Thread.Sleep(100); continue; }
                    int got;
                    bool ok2 = ReadFile(outRead, buf, Math.Min(buf.Length, avail), out got, IntPtr.Zero);
                    if (!ok2 || got == 0) break;
                    screen.Append(Encoding.UTF8.GetString(buf, 0, got));
                }
                return screen.ToString();
            };
            drain(settleMs);
            var promptBytes = Encoding.UTF8.GetBytes(prompt);
            int w1; Check(WriteFile(inWrite, promptBytes, promptBytes.Length, out w1, IntPtr.Zero), "WriteFile(prompt)");
            var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
            while (DateTime.UtcNow < deadline && !screen.ToString().Contains(expect)) { drain(500); }
            bool found = screen.ToString().Contains(expect);
            var quitBytes = Encoding.UTF8.GetBytes(quit);
            int w2; WriteFile(inWrite, quitBytes, quitBytes.Length, out w2, IntPtr.Zero);
            drain(1500);
            if (pi.hProcess != IntPtr.Zero) { TerminateProcess(pi.hProcess, 0); WaitForSingleObject(pi.hProcess, 5000); }
            screen.Append(" [driver] token_found=" + found);
        } finally {
            if (pi.hThread != IntPtr.Zero) CloseHandle(pi.hThread);
            if (pi.hProcess != IntPtr.Zero) CloseHandle(pi.hProcess);
            DeleteProcThreadAttributeList(attrList);
            Marshal.FreeHGlobal(attrList);
            ClosePseudoConsole(hPC);
            CloseHandle(inWrite); CloseHandle(outRead);
        }
        return screen.ToString();
    }
}
'@

Add-Type -TypeDefinition $source

if (Test-Path $Root) { Remove-Item -Recurse -Force $Root }
New-Item -ItemType Directory -Force "$Root\workspace" | Out-Null
$env:NANO_HOME = "$Root\nano-home"
if (-not $env:FLUX_API_KEY_FILE) { throw 'FLUX_API_KEY_FILE not set (path only)' }

# Human behavior: app opens, user waits for the UI, types a question, hits enter,
# waits for the answer, then quits with /exit (falling back to ctrl+c handled by Terminate).
$prompt = "Reply with exactly the token TUI_HUMAN_OK and nothing else.`r"
try { $screen = [ConPtyDrive]::Drive($Bin, "$Root\workspace", $prompt, 'TUI_HUMAN_OK', "/quit`r", 2500, 45000)

} catch { "DRIVE-EXCEPTION: " + $_.Exception.InnerException.Message; $screen = $null }

if ($null -eq $screen) { "(no screen captured)"; exit 1 }
$plain = $screen -replace "`e\[[0-9;?]*[a-zA-Z]", ''
$hasToken = $plain -match 'TUI_HUMAN_OK'
$hasUi = $plain.Length -gt 200
"TUI-LEG: token_rendered=$hasToken screen_bytes=$($plain.Length)"
if ($hasToken -and $hasUi) { 'TUI-LEG: PASS' } else { 'TUI-LEG: FAIL'; $plain.Substring(0, [Math]::Min(600, $plain.Length)) }

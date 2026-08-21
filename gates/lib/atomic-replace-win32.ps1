param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination
)

$ErrorActionPreference = 'Stop'

$nativeSource = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class NanoArtifactAtomicReplace
{
    public const uint MOVEFILE_REPLACE_EXISTING = 0x00000001;

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool MoveFileExW(
        string lpExistingFileName,
        string lpNewFileName,
        uint dwFlags);

    public static void Replace(string source, string destination)
    {
        const int ERROR_ACCESS_DENIED = 5;
        const int ERROR_SHARING_VIOLATION = 32;
        DateTime deadline = DateTime.UtcNow.AddSeconds(10);
        while (!MoveFileExW(source, destination, MOVEFILE_REPLACE_EXISTING))
        {
            int error = Marshal.GetLastWin32Error();
            if ((error != ERROR_ACCESS_DENIED && error != ERROR_SHARING_VIOLATION) ||
                DateTime.UtcNow >= deadline)
            {
                throw new Win32Exception(error, "atomic replacement failed");
            }
            System.Threading.Thread.Sleep(5);
        }
    }
}
'@

try {
    Add-Type -TypeDefinition $nativeSource -Language CSharp -ErrorAction Stop
    [NanoArtifactAtomicReplace]::Replace($Source, $Destination)
}
catch {
    [Console]::Error.WriteLine('artifact writer: ATOMIC_REPLACE_FAILED')
    exit 1
}

param()
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HBInputProbe {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
  [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] private static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] private static extern bool GetLastInputInfo(ref LASTINPUTINFO info);
  public static string Sample() {
    POINT p;
    bool readable = GetCursorPos(out p);
    LASTINPUTINFO info = new LASTINPUTINFO();
    info.cbSize = (uint)Marshal.SizeOf(typeof(LASTINPUTINFO));
    bool ticked = GetLastInputInfo(ref info);
    return "{\"foreground\":" + GetForegroundWindow().ToInt64()
      + ",\"cursorX\":" + (readable ? p.X : 0)
      + ",\"cursorY\":" + (readable ? p.Y : 0)
      + ",\"cursorReadable\":" + (readable ? "true" : "false")
      + ",\"inputTick\":" + (ticked ? info.dwTime.ToString() : "null") + "}";
  }
}
'@
[Console]::Out.Write("ready`n")
[Console]::Out.Flush()
while ($true) {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { break }
  $line = $line.Trim()
  if ($line -eq 'quit') { break }
  if ($line -eq 'sample') {
    [Console]::Out.Write([HBInputProbe]::Sample() + "`n")
    [Console]::Out.Flush()
  }
}

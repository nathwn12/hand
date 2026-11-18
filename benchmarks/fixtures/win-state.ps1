param([long]$Hwnd = 0)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HBWinState {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
  [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] private static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] private static extern bool GetLastInputInfo(ref LASTINPUTINFO info);
  [DllImport("user32.dll")] private static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr h);
  public static string Read(long hwnd) {
    POINT p; GetCursorPos(out p);
    // The input tick is what attributes a pointer/foreground delta: a physical
    // event changes it, a synthetic write does not.
    LASTINPUTINFO info = new LASTINPUTINFO();
    info.cbSize = (uint)Marshal.SizeOf(typeof(LASTINPUTINFO));
    bool ticked = GetLastInputInfo(ref info);
    IntPtr h = new IntPtr(hwnd);
    bool isWindow = hwnd != 0 && IsWindow(h);
    bool visible = isWindow && IsWindowVisible(h);
    return "{\"foreground\":" + GetForegroundWindow().ToInt64()
      + ",\"cursorX\":" + p.X
      + ",\"cursorY\":" + p.Y
      + ",\"inputTick\":" + (ticked ? info.dwTime.ToString() : "null")
      + ",\"hwnd\":" + hwnd
      + ",\"isWindow\":" + (isWindow ? "true" : "false")
      + ",\"visible\":" + (visible ? "true" : "false") + "}";
  }
}
'@
[HBWinState]::Read($Hwnd)

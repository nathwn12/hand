param([long]$Hwnd = 0)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HBWinState {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] private static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] private static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr h);
  public static string Read(long hwnd) {
    POINT p; GetCursorPos(out p);
    IntPtr h = new IntPtr(hwnd);
    bool isWindow = hwnd != 0 && IsWindow(h);
    bool visible = isWindow && IsWindowVisible(h);
    return "{\"foreground\":" + GetForegroundWindow().ToInt64()
      + ",\"cursorX\":" + p.X
      + ",\"cursorY\":" + p.Y
      + ",\"hwnd\":" + hwnd
      + ",\"isWindow\":" + (isWindow ? "true" : "false")
      + ",\"visible\":" + (visible ? "true" : "false") + "}";
  }
}
'@
[HBWinState]::Read($Hwnd)

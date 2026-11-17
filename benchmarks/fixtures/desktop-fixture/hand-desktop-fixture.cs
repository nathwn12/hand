using System;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;

// Controlled 1280x720 desktop fixture for the benchmark driver.
// It is shown with WS_EX_NOACTIVATE and WinForms ShowWithoutActivation; it never
// calls SetForegroundWindow or AllowSetForegroundWindow and never runs a modal
// loop. It writes a ready record with its window identity and the foreground /
// cursor samples taken immediately before and after it is shown. The launcher
// independently samples the same state and refuses to measure if it moved.

internal static class Program
{
    private const int WS_EX_NOACTIVATE = 0x08000000;
    private const int WS_EX_TOOLWINDOW = 0x00000080;
    private const int WIDTH = 1280;
    private const int HEIGHT = 720;

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT { public int X, Y; }

    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    private static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")]
    private static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")]
    private static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr h);

    [STAThread]
    private static void Main(string[] args)
    {
        if (args.Length < 1)
        {
            Environment.ExitCode = 2;
            return;
        }
        string readyPath = args[0];
        string title = args.Length > 1 ? args[1] : "hand-bench-desktop-1280x720";

        POINT cursorBefore;
        GetCursorPos(out cursorBefore);
        long foregroundBefore = GetForegroundWindow().ToInt64();

        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);

        var form = new FixtureForm
        {
            Text = title,
            FormBorderStyle = FormBorderStyle.None,
            StartPosition = FormStartPosition.Manual,
            Location = new Point(100, 100),
            ClientSize = new Size(WIDTH, HEIGHT),
            MinimumSize = new Size(WIDTH, HEIGHT),
            MaximumSize = new Size(WIDTH, HEIGHT),
            BackColor = Color.FromArgb(245, 245, 245),
            AutoScaleMode = AutoScaleMode.None,
            ShowInTaskbar = false,
            TopMost = false,
            MaximizeBox = false,
            MinimizeBox = false,
            ControlBox = false,
        };
        AddContent(form);

        form.Shown += delegate
        {
            POINT cursorAfter;
            GetCursorPos(out cursorAfter);
            long foregroundAfter = GetForegroundWindow().ToInt64();
            RECT rect;
            RECT client;
            GetWindowRect(form.Handle, out rect);
            GetClientRect(form.Handle, out client);
            string json = new StringBuilder()
                .Append("{")
                .Append("\"pid\":").Append(System.Diagnostics.Process.GetCurrentProcess().Id).Append(",")
                .Append("\"hwnd\":").Append(form.Handle.ToInt64()).Append(",")
                .Append("\"title\":\"").Append(Escape(title)).Append("\",")
                .Append("\"rect\":[").Append(rect.Left).Append(",").Append(rect.Top).Append(",").Append(rect.Right).Append(",").Append(rect.Bottom).Append("],")
                .Append("\"clientRect\":[").Append(client.Left).Append(",").Append(client.Top).Append(",").Append(client.Right).Append(",").Append(client.Bottom).Append("],")
                .Append("\"dpi\":").Append(GetDpiForWindow(form.Handle)).Append(",")
                .Append("\"foregroundBeforeShow\":").Append(foregroundBefore).Append(",")
                .Append("\"foregroundAfterShow\":").Append(foregroundAfter).Append(",")
                .Append("\"cursorBeforeShow\":[").Append(cursorBefore.X).Append(",").Append(cursorBefore.Y).Append("],")
                .Append("\"cursorAfterShow\":[").Append(cursorAfter.X).Append(",").Append(cursorAfter.Y).Append("]")
                .Append("}")
                .ToString();
            File.WriteAllText(readyPath, json, new UTF8Encoding(false));
        };

        Application.Run(form);
    }

    private static void AddContent(Form form)
    {
        var header = new Label
        {
            Text = "hand desktop fixture 1280x720  v1",
            Font = new Font("Segoe UI", 18f, FontStyle.Bold),
            ForeColor = Color.FromArgb(20, 20, 20),
            BackColor = Color.Transparent,
            Location = new Point(20, 16),
            Size = new Size(700, 40),
        };
        form.Controls.Add(header);

        Color[] palette =
        {
            Color.FromArgb(210, 40, 50), Color.FromArgb(230, 140, 30), Color.FromArgb(210, 190, 40),
            Color.FromArgb(40, 150, 70), Color.FromArgb(30, 120, 190), Color.FromArgb(90, 70, 170),
            Color.FromArgb(160, 60, 150), Color.FromArgb(80, 80, 80),
        };
        for (int row = 0; row < 8; row++)
        {
            for (int col = 0; col < 10; col++)
            {
                var panel = new Panel
                {
                    Location = new Point(20 + col * 124, 70 + row * 76),
                    Size = new Size(120, 70),
                    BackColor = palette[(row * 10 + col) % palette.Length],
                    BorderStyle = BorderStyle.FixedSingle,
                };
                var label = new Label
                {
                    Text = "C" + (row * 10 + col + 1).ToString("000"),
                    ForeColor = Color.White,
                    BackColor = Color.Transparent,
                    Font = new Font("Segoe UI", 10f, FontStyle.Bold),
                    TextAlign = ContentAlignment.MiddleCenter,
                    Dock = DockStyle.Fill,
                };
                panel.Controls.Add(label);
                form.Controls.Add(panel);
            }
        }
        var footer = new Label
        {
            Text = "fixture-v1 | fixed content | no clock | no animation | no activation",
            Font = new Font("Segoe UI", 10f, FontStyle.Regular),
            ForeColor = Color.FromArgb(30, 30, 30),
            BackColor = Color.Transparent,
            Location = new Point(20, 690),
            Size = new Size(1000, 24),
        };
        form.Controls.Add(footer);
    }

    private static string Escape(string value)
    {
        return value.Replace("\\", "\\\\").Replace("\"", "\\\"");
    }

    private sealed class FixtureForm : Form
    {
        protected override bool ShowWithoutActivation
        {
            get { return true; }
        }

        protected override CreateParams CreateParams
        {
            get
            {
                CreateParams cp = base.CreateParams;
                cp.ExStyle |= WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
                return cp;
            }
        }
    }
}

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using System.Windows.Forms;

namespace RustshotBench
{
    public sealed class WindowInfo
    {
        public long Handle { get; set; }
        public int ProcessId { get; set; }
        public int Left { get; set; }
        public int Top { get; set; }
        public int Width { get; set; }
        public int Height { get; set; }
        public string ClassName { get; set; }
        public string Title { get; set; }
    }

    public sealed class ActivationResult
    {
        public bool Success { get; set; }
        public double ElapsedMs { get; set; }
        public long StartTimestamp { get; set; }
        public long EndTimestamp { get; set; }
        public WindowInfo Window { get; set; }
        public string Error { get; set; }
    }

    public sealed class ClipboardResult
    {
        public bool Success { get; set; }
        public double ElapsedMs { get; set; }
        public long StartTimestamp { get; set; }
        public long EndTimestamp { get; set; }
        public uint SequenceNumber { get; set; }
        public int Width { get; set; }
        public int Height { get; set; }
        public string Sha256 { get; set; }
        public double PixelMeanError { get; set; }
        public int AlignmentX { get; set; }
        public int AlignmentY { get; set; }
        public string Error { get; set; }
    }

    public sealed class ResourceResult
    {
        public long WorkingSetBytes { get; set; }
        public long PrivateBytes { get; set; }
        public int HandleCount { get; set; }
        public int GdiObjects { get; set; }
        public int UserObjects { get; set; }
        public double CpuPercent { get; set; }
        public int SampleMilliseconds { get; set; }
    }

    public sealed class PatternBounds
    {
        public int Left { get; set; }
        public int Top { get; set; }
        public int Width { get; set; }
        public int Height { get; set; }
    }

    internal struct ShortcutSpec
    {
        public ushort[] Modifiers;
        public ushort Key;
    }

    public static class NativeBench
    {
        private const uint INPUT_MOUSE = 0;
        private const uint INPUT_KEYBOARD = 1;
        private const uint KEYEVENTF_KEYUP = 0x0002;
        private const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
        private const uint MOUSEEVENTF_LEFTUP = 0x0004;
        private const int GR_GDIOBJECTS = 0;
        private const int GR_USEROBJECTS = 1;
        private const uint CF_BITMAP = 2;
        private const uint CF_DIB = 8;
        private const uint CF_DIBV5 = 17;
        private const uint BI_RGB = 0;
        private const uint BI_BITFIELDS = 3;
        private const uint BI_ALPHABITFIELDS = 6;

        private const ushort VK_BACK = 0x08;
        private const ushort VK_TAB = 0x09;
        private const ushort VK_RETURN = 0x0D;
        private const ushort VK_SHIFT = 0x10;
        private const ushort VK_CONTROL = 0x11;
        private const ushort VK_MENU = 0x12;
        private const ushort VK_ESCAPE = 0x1B;
        private const ushort VK_SPACE = 0x20;
        private const ushort VK_PRIOR = 0x21;
        private const ushort VK_NEXT = 0x22;
        private const ushort VK_END = 0x23;
        private const ushort VK_HOME = 0x24;
        private const ushort VK_LEFT = 0x25;
        private const ushort VK_UP = 0x26;
        private const ushort VK_RIGHT = 0x27;
        private const ushort VK_DOWN = 0x28;
        private const ushort VK_SNAPSHOT = 0x2C;
        private const ushort VK_INSERT = 0x2D;
        private const ushort VK_DELETE = 0x2E;
        private const ushort VK_LWIN = 0x5B;

        private delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);

        [StructLayout(LayoutKind.Sequential)]
        private struct RECT
        {
            public int Left;
            public int Top;
            public int Right;
            public int Bottom;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct INPUT
        {
            public uint type;
            public INPUTUNION data;
        }

        [StructLayout(LayoutKind.Explicit)]
        private struct INPUTUNION
        {
            [FieldOffset(0)] public MOUSEINPUT mi;
            [FieldOffset(0)] public KEYBDINPUT ki;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct MOUSEINPUT
        {
            public int dx;
            public int dy;
            public uint mouseData;
            public uint dwFlags;
            public uint time;
            public UIntPtr dwExtraInfo;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct KEYBDINPUT
        {
            public ushort wVk;
            public ushort wScan;
            public uint dwFlags;
            public uint time;
            public UIntPtr dwExtraInfo;
        }

        [DllImport("user32.dll")]
        private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

        [DllImport("user32.dll")]
        private static extern bool IsWindowVisible(IntPtr hwnd);

        [DllImport("user32.dll")]
        private static extern bool IsWindow(IntPtr hwnd);

        [DllImport("user32.dll")]
        private static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);

        [DllImport("user32.dll")]
        private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetClassName(IntPtr hwnd, StringBuilder text, int count);

        [DllImport("user32.dll")]
        private static extern uint SendInput(uint count, INPUT[] inputs, int size);

        [DllImport("user32.dll")]
        private static extern bool SetCursorPos(int x, int y);

        [DllImport("user32.dll")]
        private static extern uint GetClipboardSequenceNumber();

        [DllImport("user32.dll")]
        private static extern bool OpenClipboard(IntPtr owner);

        [DllImport("user32.dll")]
        private static extern bool CloseClipboard();

        [DllImport("user32.dll")]
        private static extern bool IsClipboardFormatAvailable(uint format);

        [DllImport("user32.dll")]
        private static extern IntPtr GetClipboardData(uint format);

        [DllImport("kernel32.dll")]
        private static extern IntPtr GlobalLock(IntPtr memory);

        [DllImport("kernel32.dll")]
        private static extern bool GlobalUnlock(IntPtr memory);

        [DllImport("kernel32.dll")]
        private static extern UIntPtr GlobalSize(IntPtr memory);

        [DllImport("user32.dll")]
        private static extern uint GetGuiResources(IntPtr process, int flags);

        [DllImport("user32.dll")]
        private static extern bool SetForegroundWindow(IntPtr hwnd);

        [DllImport("user32.dll")]
        private static extern IntPtr GetForegroundWindow();

        [DllImport("user32.dll")]
        private static extern bool SetProcessDPIAware();

        [DllImport("winmm.dll")]
        private static extern uint timeBeginPeriod(uint period);

        [DllImport("winmm.dll")]
        private static extern uint timeEndPeriod(uint period);

        public static long TimestampFrequency
        {
            get { return Stopwatch.Frequency; }
        }

        public static long Timestamp
        {
            get { return Stopwatch.GetTimestamp(); }
        }

        public static void EnableDpiAwareness()
        {
            // This must run before the benchmark creates its WinForms pattern window.
            // Failure commonly means the host already selected an awareness mode, which
            // is still preferable to silently mixing logical and physical coordinates.
            SetProcessDPIAware();
        }

        public static string ValidateShortcut(string shortcut)
        {
            ShortcutSpec spec = ParseShortcut(shortcut);
            return string.Format("{0} modifier(s), VK 0x{1:X2}", spec.Modifiers.Length, spec.Key);
        }

        public static string FormatWindowsHotkey(int modifiers, int virtualKey)
        {
            List<string> parts = new List<string>();
            // MOD_* values used by RegisterHotKey and persisted by Lightshot.
            if ((modifiers & 0x0002) != 0) parts.Add("Ctrl");
            if ((modifiers & 0x0004) != 0) parts.Add("Shift");
            if ((modifiers & 0x0001) != 0) parts.Add("Alt");
            if ((modifiers & 0x0008) != 0) parts.Add("Win");
            parts.Add(VirtualKeyName((ushort)virtualKey));
            return string.Join("+", parts.ToArray());
        }

        public static uint ClipboardSequenceNumber
        {
            get { return GetClipboardSequenceNumber(); }
        }

        public static void SetClipboardMarker(string marker)
        {
            Clipboard.SetText(marker ?? string.Empty, TextDataFormat.UnicodeText);
        }

        public static void ActivateWindow(long handle)
        {
            SetForegroundWindow(new IntPtr(handle));
        }

        public static void MoveCursor(int x, int y)
        {
            if (!SetCursorPos(x, y))
            {
                throw new InvalidOperationException("SetCursorPos failed");
            }
        }

        public static void SendShortcut(string shortcut)
        {
            SendShortcutInternal(ParseShortcut(shortcut));
        }

        public static void SendEscape()
        {
            SendShortcut("Escape");
        }

        public static ActivationResult MeasureActivation(
            int processId,
            string shortcut,
            int timeoutMilliseconds,
            int minimumWidth,
            int minimumHeight)
        {
            HashSet<long> baseline = VisibleHandles(processId);
            ShortcutSpec spec;
            try
            {
                spec = ParseShortcut(shortcut);
            }
            catch (Exception error)
            {
                return new ActivationResult { Error = error.Message };
            }

            timeBeginPeriod(1);
            try
            {
                long start = Stopwatch.GetTimestamp();
                try
                {
                    SendShortcutInternal(spec);
                }
                catch (Exception error)
                {
                    return new ActivationResult
                    {
                        StartTimestamp = start,
                        EndTimestamp = Stopwatch.GetTimestamp(),
                        Error = error.Message
                    };
                }

                while (ElapsedMilliseconds(start, Stopwatch.GetTimestamp()) <= timeoutMilliseconds)
                {
                    WindowInfo window = FindOverlay(processId, minimumWidth, minimumHeight, baseline);
                    if (window == null)
                    {
                        window = DescribeForegroundOverlay(processId, minimumWidth, minimumHeight);
                    }
                    if (window != null)
                    {
                        long end = Stopwatch.GetTimestamp();
                        return new ActivationResult
                        {
                            Success = true,
                            ElapsedMs = ElapsedMilliseconds(start, end),
                            StartTimestamp = start,
                            EndTimestamp = end,
                            Window = window
                        };
                    }
                    Thread.Sleep(1);
                }

                long timedOut = Stopwatch.GetTimestamp();
                return new ActivationResult
                {
                    StartTimestamp = start,
                    EndTimestamp = timedOut,
                    ElapsedMs = ElapsedMilliseconds(start, timedOut),
                    Error = "timed out waiting for a new visible overlay window"
                };
            }
            finally
            {
                timeEndPeriod(1);
            }
        }

        public static WindowInfo FindAnyOverlay(int processId, int minimumWidth, int minimumHeight)
        {
            return FindOverlay(processId, minimumWidth, minimumHeight, null);
        }

        public static bool WaitForWindowGone(long handle, int timeoutMilliseconds)
        {
            IntPtr hwnd = new IntPtr(handle);
            long start = Stopwatch.GetTimestamp();
            while (ElapsedMilliseconds(start, Stopwatch.GetTimestamp()) <= timeoutMilliseconds)
            {
                if (!IsWindow(hwnd) || !IsWindowVisible(hwnd))
                {
                    return true;
                }
                Thread.Sleep(5);
            }
            return !IsWindow(hwnd) || !IsWindowVisible(hwnd);
        }

        public static long SendMouseDrag(int startX, int startY, int endX, int endY, int durationMilliseconds)
        {
            if (!SetCursorPos(startX, startY))
            {
                throw new InvalidOperationException("SetCursorPos failed at the drag start");
            }
            Thread.Sleep(20);
            SendMouseButton(MOUSEEVENTF_LEFTDOWN);

            int steps = Math.Max(2, Math.Min(60, durationMilliseconds / 4));
            int delay = Math.Max(1, durationMilliseconds / steps);
            for (int step = 1; step <= steps; step++)
            {
                int x = startX + ((endX - startX) * step / steps);
                int y = startY + ((endY - startY) * step / steps);
                SetCursorPos(x, y);
                Thread.Sleep(delay);
            }

            // Both applications consume the latest pointer position from their
            // window event queues when the button is released. Give the final
            // WM_MOUSEMOVE time to arrive so the selection cannot end one
            // interpolation step early under a busy desktop.
            Thread.Sleep(30);
            SendMouseButton(MOUSEEVENTF_LEFTUP);
            return Stopwatch.GetTimestamp();
        }

        public static void SendMouseClick(int x, int y)
        {
            if (!SetCursorPos(x, y))
            {
                throw new InvalidOperationException("SetCursorPos failed before a mouse click");
            }
            Thread.Sleep(20);
            SendMouseButton(MOUSEEVENTF_LEFTDOWN);
            Thread.Sleep(20);
            SendMouseButton(MOUSEEVENTF_LEFTUP);
        }

        public static ClipboardResult MeasureClipboardCopy(
            string copyShortcut,
            uint previousSequenceNumber,
            int timeoutMilliseconds,
            int expectedX,
            int expectedY)
        {
            ShortcutSpec spec;
            try
            {
                spec = ParseShortcut(copyShortcut);
            }
            catch (Exception error)
            {
                return new ClipboardResult { Error = error.Message };
            }

            long start = Stopwatch.GetTimestamp();
            try
            {
                SendShortcutInternal(spec);
            }
            catch (Exception error)
            {
                return new ClipboardResult
                {
                    StartTimestamp = start,
                    EndTimestamp = Stopwatch.GetTimestamp(),
                    Error = error.Message
                };
            }

            return WaitForClipboardImage(
                previousSequenceNumber,
                timeoutMilliseconds,
                expectedX,
                expectedY,
                start);
        }

        public static ClipboardResult WaitForClipboardImage(
            uint previousSequenceNumber,
            int timeoutMilliseconds,
            int expectedX,
            int expectedY,
            long startTimestamp)
        {
            long start = startTimestamp == 0 ? Stopwatch.GetTimestamp() : startTimestamp;

            string lastError = null;
            while (ElapsedMilliseconds(start, Stopwatch.GetTimestamp()) <= timeoutMilliseconds)
            {
                uint current = GetClipboardSequenceNumber();
                if (current != previousSequenceNumber)
                {
                    try
                    {
                        using (Bitmap image = ReadClipboardBitmap())
                        {
                            if (image != null)
                            {
                                long end = Stopwatch.GetTimestamp();
                                ClipboardResult result = AnalyzeImage(image, expectedX, expectedY);
                                result.Success = true;
                                result.SequenceNumber = current;
                                result.StartTimestamp = start;
                                result.EndTimestamp = end;
                                result.ElapsedMs = ElapsedMilliseconds(start, end);
                                return result;
                            }
                        }
                    }
                    catch (ExternalException error)
                    {
                        lastError = error.Message;
                    }
                    catch (InvalidOperationException error)
                    {
                        lastError = error.Message;
                    }
                }
                Thread.Sleep(2);
            }

            long timedOut = Stopwatch.GetTimestamp();
            uint finalSequence = GetClipboardSequenceNumber();
            string timeoutDetail = finalSequence == previousSequenceNumber
                ? string.Format(
                    "clipboard sequence did not change (still {0})",
                    previousSequenceNumber)
                : string.Format(
                    "clipboard changed from sequence {0} to {1}, but no readable image appeared",
                    previousSequenceNumber,
                    finalSequence);
            return new ClipboardResult
            {
                StartTimestamp = start,
                EndTimestamp = timedOut,
                ElapsedMs = ElapsedMilliseconds(start, timedOut),
                SequenceNumber = finalSequence,
                Error = lastError == null
                    ? "timed out: " + timeoutDetail
                    : "timed out reading the clipboard image (" + timeoutDetail + "): " + lastError
            };
        }

        public static ResourceResult SampleResources(Process process, int sampleMilliseconds)
        {
            process.Refresh();
            TimeSpan cpuStart = process.TotalProcessorTime;
            Thread.Sleep(sampleMilliseconds);
            process.Refresh();
            TimeSpan cpuEnd = process.TotalProcessorTime;
            double capacity = sampleMilliseconds * Math.Max(1, Environment.ProcessorCount);
            double cpuPercent = (cpuEnd - cpuStart).TotalMilliseconds * 100.0 / capacity;
            return new ResourceResult
            {
                WorkingSetBytes = process.WorkingSet64,
                PrivateBytes = process.PrivateMemorySize64,
                HandleCount = process.HandleCount,
                GdiObjects = unchecked((int)GetGuiResources(process.Handle, GR_GDIOBJECTS)),
                UserObjects = unchecked((int)GetGuiResources(process.Handle, GR_USEROBJECTS)),
                CpuPercent = cpuPercent,
                SampleMilliseconds = sampleMilliseconds
            };
        }

        public static double MillisecondsBetween(long startTimestamp, long endTimestamp)
        {
            return ElapsedMilliseconds(startTimestamp, endTimestamp);
        }

        private static ClipboardResult AnalyzeImage(Image source, int expectedX, int expectedY)
        {
            using (Bitmap bitmap = new Bitmap(source.Width, source.Height, PixelFormat.Format32bppArgb))
            {
                using (Graphics graphics = Graphics.FromImage(bitmap))
                {
                    graphics.DrawImageUnscaled(source, 0, 0);
                }

                int bestX = 0;
                int bestY = 0;
                double bestError = double.MaxValue;
                for (int offsetY = -3; offsetY <= 3; offsetY++)
                {
                    for (int offsetX = -3; offsetX <= 3; offsetX++)
                    {
                        double error = SamplePatternError(bitmap, expectedX + offsetX, expectedY + offsetY);
                        if (error < bestError)
                        {
                            bestError = error;
                            bestX = offsetX;
                            bestY = offsetY;
                        }
                    }
                }

                return new ClipboardResult
                {
                    Width = bitmap.Width,
                    Height = bitmap.Height,
                    Sha256 = HashBitmap(bitmap),
                    PixelMeanError = bestError,
                    AlignmentX = bestX,
                    AlignmentY = bestY
                };
            }
        }

        private static Bitmap ReadClipboardBitmap()
        {
            for (int attempt = 0; attempt < 10; attempt++)
            {
                if (OpenClipboard(IntPtr.Zero))
                {
                    try
                    {
                        if (IsClipboardFormatAvailable(CF_DIBV5))
                        {
                            Bitmap dibv5 = ReadDibHandle(GetClipboardData(CF_DIBV5));
                            if (dibv5 != null) return dibv5;
                        }
                        if (IsClipboardFormatAvailable(CF_DIB))
                        {
                            Bitmap dib = ReadDibHandle(GetClipboardData(CF_DIB));
                            if (dib != null) return dib;
                        }
                    }
                    finally
                    {
                        CloseClipboard();
                    }
                    break;
                }
                Thread.Sleep(2);
            }

            // Some applications publish only CF_BITMAP. System.Windows.Forms can
            // clone that GDI handle safely once our direct clipboard lock is gone.
            if (IsClipboardFormatAvailable(CF_BITMAP) || Clipboard.ContainsImage())
            {
                using (Image image = Clipboard.GetImage())
                {
                    if (image != null)
                    {
                        return new Bitmap(image);
                    }
                }
            }
            return null;
        }

        private static Bitmap ReadDibHandle(IntPtr handle)
        {
            if (handle == IntPtr.Zero)
            {
                return null;
            }
            long size = unchecked((long)GlobalSize(handle).ToUInt64());
            if (size < 40 || size > int.MaxValue)
            {
                throw new InvalidOperationException("clipboard DIB has an invalid allocation size");
            }
            IntPtr baseAddress = GlobalLock(handle);
            if (baseAddress == IntPtr.Zero)
            {
                throw new InvalidOperationException("could not lock the clipboard DIB");
            }
            try
            {
                int headerSize = Marshal.ReadInt32(baseAddress, 0);
                int signedWidth = Marshal.ReadInt32(baseAddress, 4);
                int signedHeight = Marshal.ReadInt32(baseAddress, 8);
                ushort bitCount = unchecked((ushort)Marshal.ReadInt16(baseAddress, 14));
                uint compression = unchecked((uint)Marshal.ReadInt32(baseAddress, 16));
                uint colorsUsed = unchecked((uint)Marshal.ReadInt32(baseAddress, 32));
                int width = Math.Abs(signedWidth);
                int height = Math.Abs(signedHeight);
                if (headerSize < 40 || headerSize > size || width < 1 || height < 1)
                {
                    throw new InvalidOperationException("clipboard DIB header is invalid");
                }
                if ((bitCount != 24 && bitCount != 32) ||
                    (compression != BI_RGB && compression != BI_BITFIELDS && compression != BI_ALPHABITFIELDS))
                {
                    throw new InvalidOperationException(
                        string.Format("unsupported clipboard DIB: {0} bpp, compression {1}", bitCount, compression));
                }

                long pixelOffset = headerSize;
                if (headerSize == 40 && (compression == BI_BITFIELDS || compression == BI_ALPHABITFIELDS))
                {
                    pixelOffset += compression == BI_ALPHABITFIELDS ? 16 : 12;
                }
                if (bitCount <= 8)
                {
                    long colorCount = colorsUsed == 0 ? 1L << bitCount : colorsUsed;
                    pixelOffset += colorCount * 4;
                }
                long sourceStride = ((long)width * bitCount + 31) / 32 * 4;
                long required = pixelOffset + sourceStride * height;
                if (required > size)
                {
                    throw new InvalidOperationException("clipboard DIB pixel data is truncated");
                }

                Bitmap bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
                Rectangle rectangle = new Rectangle(0, 0, width, height);
                BitmapData destination = bitmap.LockBits(
                    rectangle,
                    ImageLockMode.WriteOnly,
                    PixelFormat.Format32bppArgb);
                try
                {
                    byte[] output = new byte[Math.Abs(destination.Stride) * height];
                    int bytesPerPixel = bitCount / 8;
                    bool topDown = signedHeight < 0;
                    for (int y = 0; y < height; y++)
                    {
                        int sourceY = topDown ? y : height - 1 - y;
                        long sourceRow = pixelOffset + sourceY * sourceStride;
                        int destinationRow = y * destination.Stride;
                        for (int x = 0; x < width; x++)
                        {
                            int sourcePixel = checked((int)(sourceRow + x * bytesPerPixel));
                            int destinationPixel = destinationRow + x * 4;
                            output[destinationPixel] = Marshal.ReadByte(baseAddress, sourcePixel);
                            output[destinationPixel + 1] = Marshal.ReadByte(baseAddress, sourcePixel + 1);
                            output[destinationPixel + 2] = Marshal.ReadByte(baseAddress, sourcePixel + 2);
                            output[destinationPixel + 3] = bytesPerPixel == 4 && compression != BI_RGB
                                ? Marshal.ReadByte(baseAddress, sourcePixel + 3)
                                : (byte)255;
                        }
                    }
                    Marshal.Copy(output, 0, destination.Scan0, output.Length);
                }
                finally
                {
                    bitmap.UnlockBits(destination);
                }
                return bitmap;
            }
            finally
            {
                GlobalUnlock(handle);
            }
        }

        private static double SamplePatternError(Bitmap bitmap, int expectedX, int expectedY)
        {
            long error = 0;
            int samples = 0;
            const int grid = 12;
            for (int row = 1; row <= grid; row++)
            {
                int y = row * Math.Max(1, bitmap.Height - 1) / (grid + 1);
                for (int column = 1; column <= grid; column++)
                {
                    int x = column * Math.Max(1, bitmap.Width - 1) / (grid + 1);
                    Color actual = bitmap.GetPixel(x, y);
                    Color expected = PatternForm.PatternColor(expectedX + x, expectedY + y);
                    error += Math.Abs(actual.R - expected.R);
                    error += Math.Abs(actual.G - expected.G);
                    error += Math.Abs(actual.B - expected.B);
                    samples += 3;
                }
            }
            return samples == 0 ? double.MaxValue : (double)error / samples;
        }

        private static string HashBitmap(Bitmap bitmap)
        {
            Rectangle rectangle = new Rectangle(0, 0, bitmap.Width, bitmap.Height);
            BitmapData data = bitmap.LockBits(rectangle, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            try
            {
                int bytes = Math.Abs(data.Stride) * data.Height;
                byte[] pixels = new byte[bytes];
                Marshal.Copy(data.Scan0, pixels, 0, bytes);
                using (SHA256 hash = SHA256.Create())
                {
                    byte[] digest = hash.ComputeHash(pixels);
                    StringBuilder text = new StringBuilder(digest.Length * 2);
                    foreach (byte value in digest)
                    {
                        text.Append(value.ToString("x2"));
                    }
                    return text.ToString();
                }
            }
            finally
            {
                bitmap.UnlockBits(data);
            }
        }

        private static WindowInfo FindOverlay(
            int processId,
            int minimumWidth,
            int minimumHeight,
            HashSet<long> excludedHandles)
        {
            WindowInfo best = null;
            long bestArea = -1;
            EnumWindows(delegate(IntPtr hwnd, IntPtr unused)
            {
                uint owner;
                GetWindowThreadProcessId(hwnd, out owner);
                if (owner != (uint)processId || !IsWindowVisible(hwnd))
                {
                    return true;
                }
                long handle = hwnd.ToInt64();
                if (excludedHandles != null && excludedHandles.Contains(handle))
                {
                    return true;
                }
                RECT rect;
                if (!GetWindowRect(hwnd, out rect))
                {
                    return true;
                }
                int width = Math.Max(0, rect.Right - rect.Left);
                int height = Math.Max(0, rect.Bottom - rect.Top);
                if (width < minimumWidth || height < minimumHeight)
                {
                    return true;
                }
                long area = (long)width * height;
                if (area > bestArea)
                {
                    bestArea = area;
                    best = DescribeWindow(hwnd, processId, rect);
                }
                return true;
            }, IntPtr.Zero);
            return best;
        }

        private static WindowInfo DescribeForegroundOverlay(
            int processId,
            int minimumWidth,
            int minimumHeight)
        {
            IntPtr hwnd = GetForegroundWindow();
            if (hwnd == IntPtr.Zero || !IsWindowVisible(hwnd))
            {
                return null;
            }
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner != (uint)processId)
            {
                return null;
            }
            RECT rect;
            if (!GetWindowRect(hwnd, out rect))
            {
                return null;
            }
            int width = Math.Max(0, rect.Right - rect.Left);
            int height = Math.Max(0, rect.Bottom - rect.Top);
            if (width < minimumWidth || height < minimumHeight)
            {
                return null;
            }
            return DescribeWindow(hwnd, processId, rect);
        }

        private static HashSet<long> VisibleHandles(int processId)
        {
            HashSet<long> handles = new HashSet<long>();
            EnumWindows(delegate(IntPtr hwnd, IntPtr unused)
            {
                uint owner;
                GetWindowThreadProcessId(hwnd, out owner);
                if (owner == (uint)processId && IsWindowVisible(hwnd))
                {
                    handles.Add(hwnd.ToInt64());
                }
                return true;
            }, IntPtr.Zero);
            return handles;
        }

        private static WindowInfo DescribeWindow(IntPtr hwnd, int processId, RECT rect)
        {
            StringBuilder title = new StringBuilder(512);
            StringBuilder className = new StringBuilder(256);
            GetWindowText(hwnd, title, title.Capacity);
            GetClassName(hwnd, className, className.Capacity);
            return new WindowInfo
            {
                Handle = hwnd.ToInt64(),
                ProcessId = processId,
                Left = rect.Left,
                Top = rect.Top,
                Width = Math.Max(0, rect.Right - rect.Left),
                Height = Math.Max(0, rect.Bottom - rect.Top),
                ClassName = className.ToString(),
                Title = title.ToString()
            };
        }

        private static void SendMouseButton(uint flag)
        {
            INPUT input = new INPUT();
            input.type = INPUT_MOUSE;
            input.data.mi.dwFlags = flag;
            uint sent = SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT)));
            if (sent != 1)
            {
                throw new InvalidOperationException("SendInput failed while sending a mouse button");
            }
        }

        private static void SendShortcutInternal(ShortcutSpec shortcut)
        {
            List<INPUT> inputs = new List<INPUT>();
            foreach (ushort modifier in shortcut.Modifiers)
            {
                inputs.Add(KeyboardInput(modifier, false));
            }
            inputs.Add(KeyboardInput(shortcut.Key, false));
            inputs.Add(KeyboardInput(shortcut.Key, true));
            for (int index = shortcut.Modifiers.Length - 1; index >= 0; index--)
            {
                inputs.Add(KeyboardInput(shortcut.Modifiers[index], true));
            }
            INPUT[] array = inputs.ToArray();
            uint sent = SendInput((uint)array.Length, array, Marshal.SizeOf(typeof(INPUT)));
            if (sent != array.Length)
            {
                throw new InvalidOperationException(
                    string.Format("SendInput accepted {0} of {1} keyboard events", sent, array.Length));
            }
        }

        private static INPUT KeyboardInput(ushort virtualKey, bool keyUp)
        {
            INPUT input = new INPUT();
            input.type = INPUT_KEYBOARD;
            input.data.ki.wVk = virtualKey;
            input.data.ki.dwFlags = keyUp ? KEYEVENTF_KEYUP : 0;
            return input;
        }

        private static ShortcutSpec ParseShortcut(string shortcut)
        {
            if (string.IsNullOrWhiteSpace(shortcut))
            {
                throw new ArgumentException("hotkey cannot be empty");
            }

            List<ushort> modifiers = new List<ushort>();
            ushort key = 0;
            string[] tokens = shortcut.Split(new[] { '+', ' ' }, StringSplitOptions.RemoveEmptyEntries);
            foreach (string rawToken in tokens)
            {
                string token = rawToken.Trim().ToUpperInvariant();
                ushort modifier;
                if (TryModifier(token, out modifier))
                {
                    if (!modifiers.Contains(modifier))
                    {
                        modifiers.Add(modifier);
                    }
                    continue;
                }

                if (key != 0)
                {
                    throw new ArgumentException("hotkey must contain exactly one non-modifier key: " + shortcut);
                }
                key = VirtualKey(token);
            }

            if (key == 0)
            {
                throw new ArgumentException("hotkey has no non-modifier key: " + shortcut);
            }
            return new ShortcutSpec { Modifiers = modifiers.ToArray(), Key = key };
        }

        private static bool TryModifier(string token, out ushort key)
        {
            if (token == "CTRL" || token == "CONTROL")
            {
                key = VK_CONTROL;
                return true;
            }
            if (token == "SHIFT")
            {
                key = VK_SHIFT;
                return true;
            }
            if (token == "ALT")
            {
                key = VK_MENU;
                return true;
            }
            if (token == "WIN" || token == "WINDOWS" || token == "SUPER")
            {
                key = VK_LWIN;
                return true;
            }
            key = 0;
            return false;
        }

        private static ushort VirtualKey(string token)
        {
            if (token.StartsWith("KEY", StringComparison.Ordinal) && token.Length == 4)
            {
                token = token.Substring(3);
            }
            if (token.Length == 1)
            {
                char character = token[0];
                if ((character >= 'A' && character <= 'Z') || (character >= '0' && character <= '9'))
                {
                    return character;
                }
            }
            if (token.StartsWith("F", StringComparison.Ordinal) && token.Length <= 3)
            {
                int number;
                if (int.TryParse(token.Substring(1), out number) && number >= 1 && number <= 24)
                {
                    return (ushort)(0x70 + number - 1);
                }
            }

            switch (token)
            {
                case "ESC":
                case "ESCAPE": return VK_ESCAPE;
                case "PRINTSCREEN":
                case "PRTSC":
                case "SNAPSHOT": return VK_SNAPSHOT;
                case "ENTER":
                case "RETURN": return VK_RETURN;
                case "SPACE": return VK_SPACE;
                case "TAB": return VK_TAB;
                case "BACKSPACE": return VK_BACK;
                case "DELETE":
                case "DEL": return VK_DELETE;
                case "INSERT":
                case "INS": return VK_INSERT;
                case "HOME": return VK_HOME;
                case "END": return VK_END;
                case "PAGEUP":
                case "PGUP": return VK_PRIOR;
                case "PAGEDOWN":
                case "PGDN": return VK_NEXT;
                case "LEFT": return VK_LEFT;
                case "RIGHT": return VK_RIGHT;
                case "UP": return VK_UP;
                case "DOWN": return VK_DOWN;
                case "BRACKETLEFT":
                case "OPENBRACKET": return 0xDB;
                case "BACKSLASH": return 0xDC;
                case "BRACKETRIGHT":
                case "CLOSEBRACKET": return 0xDD;
                case "SEMICOLON": return 0xBA;
                case "QUOTE": return 0xDE;
                case "COMMA": return 0xBC;
                case "PERIOD": return 0xBE;
                case "SLASH": return 0xBF;
                case "BACKQUOTE":
                case "BACKTICK": return 0xC0;
                case "MINUS": return 0xBD;
                case "EQUAL": return 0xBB;
                default: throw new ArgumentException("unsupported hotkey key: " + token);
            }
        }

        private static string VirtualKeyName(ushort virtualKey)
        {
            if ((virtualKey >= 'A' && virtualKey <= 'Z') ||
                (virtualKey >= '0' && virtualKey <= '9'))
            {
                return ((char)virtualKey).ToString();
            }
            if (virtualKey >= 0x70 && virtualKey <= 0x87)
            {
                return "F" + (virtualKey - 0x70 + 1).ToString();
            }
            switch (virtualKey)
            {
                case VK_ESCAPE: return "Escape";
                case VK_SNAPSHOT: return "PrintScreen";
                case VK_RETURN: return "Enter";
                case VK_SPACE: return "Space";
                case VK_TAB: return "Tab";
                case VK_BACK: return "Backspace";
                case VK_DELETE: return "Delete";
                case VK_INSERT: return "Insert";
                case VK_HOME: return "Home";
                case VK_END: return "End";
                case VK_PRIOR: return "PageUp";
                case VK_NEXT: return "PageDown";
                case VK_LEFT: return "Left";
                case VK_RIGHT: return "Right";
                case VK_UP: return "Up";
                case VK_DOWN: return "Down";
                case 0xDB: return "BracketLeft";
                case 0xDC: return "Backslash";
                case 0xDD: return "BracketRight";
                case 0xBA: return "Semicolon";
                case 0xDE: return "Quote";
                case 0xBC: return "Comma";
                case 0xBE: return "Period";
                case 0xBF: return "Slash";
                case 0xC0: return "Backquote";
                case 0xBD: return "Minus";
                case 0xBB: return "Equal";
                default:
                    throw new ArgumentException(
                        string.Format("unsupported persisted virtual key: 0x{0:X2}", virtualKey));
            }
        }

        private static double ElapsedMilliseconds(long start, long end)
        {
            return (end - start) * 1000.0 / Stopwatch.Frequency;
        }
    }

    internal sealed class PatternForm : Form
    {
        private readonly Bitmap pattern;

        public PatternForm(int width, int height)
        {
            Text = "Rustshot benchmark pattern - keep unobscured";
            FormBorderStyle = FormBorderStyle.None;
            ShowInTaskbar = true;
            StartPosition = FormStartPosition.Manual;
            ClientSize = new Size(width, height);
            BackColor = Color.Black;
            pattern = CreatePattern(width, height);
            SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.UserPaint | ControlStyles.Opaque, true);
        }

        protected override void OnPaint(PaintEventArgs eventArgs)
        {
            eventArgs.Graphics.DrawImageUnscaled(pattern, 0, 0);
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                pattern.Dispose();
            }
            base.Dispose(disposing);
        }

        public static Color PatternColor(int x, int y)
        {
            unchecked
            {
                int red = (x * 3 + y * 5) & 0xFF;
                int green = (x * 7 + y * 11) & 0xFF;
                int blue = (x * 13 + y * 17) & 0xFF;
                return Color.FromArgb(255, red, green, blue);
            }
        }

        private static Bitmap CreatePattern(int width, int height)
        {
            Bitmap bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
            Rectangle rectangle = new Rectangle(0, 0, width, height);
            BitmapData data = bitmap.LockBits(rectangle, ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
            try
            {
                byte[] pixels = new byte[Math.Abs(data.Stride) * height];
                for (int y = 0; y < height; y++)
                {
                    for (int x = 0; x < width; x++)
                    {
                        Color color = PatternColor(x, y);
                        int offset = y * data.Stride + x * 4;
                        pixels[offset] = color.B;
                        pixels[offset + 1] = color.G;
                        pixels[offset + 2] = color.R;
                        pixels[offset + 3] = 255;
                    }
                }
                Marshal.Copy(pixels, 0, data.Scan0, pixels.Length);
            }
            finally
            {
                bitmap.UnlockBits(data);
            }
            return bitmap;
        }
    }

    public sealed class PatternHost : IDisposable
    {
        private readonly int width;
        private readonly int height;
        private readonly ManualResetEvent ready = new ManualResetEvent(false);
        private Thread thread;
        private PatternForm form;
        private Exception startError;

        public PatternHost(int width, int height)
        {
            this.width = width;
            this.height = height;
        }

        public void Start()
        {
            if (thread != null)
            {
                return;
            }
            thread = new Thread(Run);
            thread.Name = "Rustshot benchmark pattern";
            thread.IsBackground = true;
            thread.SetApartmentState(ApartmentState.STA);
            thread.Start();
            if (!ready.WaitOne(5000))
            {
                throw new TimeoutException("test-pattern window did not start within five seconds");
            }
            if (startError != null)
            {
                throw new InvalidOperationException("test-pattern window failed to start", startError);
            }
        }

        public PatternBounds Bounds
        {
            get
            {
                EnsureStarted();
                return (PatternBounds)form.Invoke(new Func<PatternBounds>(delegate
                {
                    Point origin = form.PointToScreen(Point.Empty);
                    return new PatternBounds
                    {
                        Left = origin.X,
                        Top = origin.Y,
                        Width = form.ClientSize.Width,
                        Height = form.ClientSize.Height
                    };
                }));
            }
        }

        public void Activate()
        {
            EnsureStarted();
            form.Invoke(new Action(delegate
            {
                if (form.WindowState == FormWindowState.Minimized)
                {
                    form.WindowState = FormWindowState.Normal;
                }
                form.BringToFront();
                form.Activate();
                NativeBench.ActivateWindow(form.Handle.ToInt64());
                form.Invalidate();
                form.Update();
            }));
        }

        public void Dispose()
        {
            if (form != null && !form.IsDisposed)
            {
                try
                {
                    form.BeginInvoke(new Action(delegate { form.Close(); }));
                }
                catch (InvalidOperationException)
                {
                }
            }
            if (thread != null && thread.IsAlive)
            {
                thread.Join(3000);
            }
            ready.Dispose();
        }

        private void Run()
        {
            try
            {
                form = new PatternForm(width, height);
                Rectangle working = Screen.PrimaryScreen.WorkingArea;
                form.Location = new Point(
                    working.Left + Math.Max(0, (working.Width - width) / 2),
                    working.Top + Math.Max(0, (working.Height - height) / 2));
                form.Shown += delegate { ready.Set(); };
                Application.Run(form);
            }
            catch (Exception error)
            {
                startError = error;
                ready.Set();
            }
        }

        private void EnsureStarted()
        {
            if (thread == null || form == null || form.IsDisposed)
            {
                throw new InvalidOperationException("test-pattern window is not running");
            }
        }
    }
}

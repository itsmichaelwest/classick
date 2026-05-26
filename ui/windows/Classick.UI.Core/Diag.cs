using System;
using System.Diagnostics;
using System.IO;

namespace Classick_UI.Core;

/// <summary>
/// Lightweight diagnostic logger that writes to BOTH OutputDebugString (for
/// devs with a debugger / DebugView++ attached) AND a rotating file under
/// <c>%LOCALAPPDATA%\classick\logs\ui-{yyyyMMddTHHmmss}.log</c>.
///
/// The file path mirrors the core's IPC log path so all crash diagnostics
/// live in one folder, with a <c>ui-</c> prefix vs the core's <c>core-</c>
/// prefix. Initialized lazily on first use; thread-safe.
/// </summary>
public static class Diag
{
    private static readonly object Lock = new();
    private static string? _path;

    private static string GetPath()
    {
        if (_path is not null) return _path;
        lock (Lock)
        {
            if (_path is not null) return _path;
            var baseDir = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                AppIdentity.Name, "logs");
            try { Directory.CreateDirectory(baseDir); } catch { /* ignore */ }
            var ts = DateTime.Now.ToString("yyyyMMddTHHmmss");
            _path = Path.Combine(baseDir, $"ui-{ts}.log");
            return _path;
        }
    }

    public static void Log(string message)
    {
        var line = $"{DateTime.Now:HH:mm:ss.fff} {message}";
        Debug.WriteLine(line);
        try
        {
            lock (Lock)
            {
                File.AppendAllText(GetPath(), line + Environment.NewLine);
            }
        }
        catch
        {
            // Diagnostic logging must never throw; swallow.
        }
    }
}

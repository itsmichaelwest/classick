using System;
using System.Collections.Generic;
using System.IO;

namespace IpodSync_UI.Core;

/// <summary>
/// Locates <c>ipod-sync.exe</c> on disk. Resolution order:
/// 1. Explicit path passed to <see cref="Find(string?)"/> (e.g. from config).
/// 2. Sibling to the UI exe (production install layout).
/// 3. <c>..\..\target\release\ipod-sync.exe</c> relative to the UI exe (dev layout).
/// 4. <c>..\..\target\debug\ipod-sync.exe</c> (dev layout, debug build).
/// 5. <c>ipod-sync.exe</c> on the system PATH.
///
/// Returns the first path that exists; throws <see cref="CoreNotFoundException"/>
/// with a multi-line message listing every location tried if nothing is found.
/// </summary>
public static class CoreLocator
{
    private const string CoreExeName = "ipod-sync.exe";

    /// <summary>
    /// Find the core executable. <paramref name="explicitPath"/> takes
    /// precedence if non-null and the file exists; otherwise the lookup
    /// chain runs.
    /// </summary>
    public static string Find(string? explicitPath = null)
    {
        var tried = new List<string>();

        // 1. Explicit path (e.g. from persisted config or a flag).
        if (!string.IsNullOrWhiteSpace(explicitPath))
        {
            tried.Add($"--core-path argument: {explicitPath}");
            if (File.Exists(explicitPath)) return Path.GetFullPath(explicitPath);
        }

        // 2 + 3 + 4. Paths relative to the UI exe.
        var uiExeDir = GetUiExeDirectory();
        if (uiExeDir is not null)
        {
            string[] relativeCandidates = new[]
            {
                Path.Combine(uiExeDir, CoreExeName),                                   // sibling
                Path.Combine(uiExeDir, "..", "..", "target", "release", CoreExeName), // dev release
                Path.Combine(uiExeDir, "..", "..", "target", "debug", CoreExeName),   // dev debug
            };
            foreach (var candidate in relativeCandidates)
            {
                var full = Path.GetFullPath(candidate);
                tried.Add(full);
                if (File.Exists(full)) return full;
            }
        }

        // 5. PATH lookup.
        var pathHit = FindOnPath(CoreExeName);
        if (pathHit is not null)
        {
            tried.Add($"PATH: {pathHit}");
            return pathHit;
        }
        else
        {
            tried.Add("(not found on PATH)");
        }

        throw new CoreNotFoundException(tried);
    }

    private static string? GetUiExeDirectory()
    {
        // AppContext.BaseDirectory is the running exe's directory in published
        // apps; in dev under `dotnet run` it's bin\Debug\net10.0-windows10.0.*\.
        // Either way it's the right anchor for relative lookups.
        var baseDir = AppContext.BaseDirectory;
        return string.IsNullOrEmpty(baseDir) ? null : baseDir;
    }

    private static string? FindOnPath(string exeName)
    {
        var pathEnv = Environment.GetEnvironmentVariable("PATH");
        if (string.IsNullOrEmpty(pathEnv)) return null;
        foreach (var dir in pathEnv.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            try
            {
                var candidate = Path.Combine(dir, exeName);
                if (File.Exists(candidate)) return candidate;
            }
            catch { /* invalid path entry; skip */ }
        }
        return null;
    }
}

public sealed class CoreNotFoundException : Exception
{
    public IReadOnlyList<string> LocationsTried { get; }

    public CoreNotFoundException(IReadOnlyList<string> locationsTried)
        : base(BuildMessage(locationsTried))
    {
        LocationsTried = locationsTried;
    }

    private static string BuildMessage(IReadOnlyList<string> tried)
    {
        var lines = new List<string>
        {
            "ipod-sync.exe was not found.",
            "",
            "Tried (in order):",
        };
        for (int i = 0; i < tried.Count; i++)
        {
            lines.Add($"  {i + 1}. {tried[i]}");
        }
        lines.Add("");
        lines.Add("To fix:");
        lines.Add("  - Build the Rust core: cd to the repo root and run `cargo build --release`,");
        lines.Add("    then place the resulting target\\release\\ipod-sync.exe next to this UI exe,");
        lines.Add("    OR add the build's containing directory to your PATH.");
        return string.Join('\n', lines);
    }
}

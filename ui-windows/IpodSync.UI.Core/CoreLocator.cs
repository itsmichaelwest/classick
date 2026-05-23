using System;
using System.Collections.Generic;
using System.IO;

namespace IpodSync_UI.Core;

/// <summary>
/// Locates <c>ipod-sync.exe</c> on disk. Resolution order:
/// 1. Explicit path passed to <see cref="Find(string?)"/> (e.g. from config).
/// 2. Sibling to the UI exe — both in dev (the IpodSync.UI csproj has a
///    <c>&lt;Content Include="..\..\target\release\ipod-sync.exe"&gt;</c>
///    item that copies the binary next to IpodSync.UI.exe at build time)
///    and in production (the MSIX/installer ships them in the same dir).
///    This is the canonical layout; the walk-up fallback that previously
///    lived here was a band-aid for missing build glue.
/// 3. <c>ipod-sync.exe</c> on the system PATH (escape hatch for unusual setups).
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

        // 2. Sibling to the UI exe.
        //    In production MSIX, the install dir IS the AppX layout and our
        //    bundled Content lives right next to IpodSync.UI.exe.
        //    In dev-time packaged WinUI, MSBuild copies non-image Content
        //    one directory ABOVE the AppX layout (..\win-x64\foo.exe while
        //    the running exe is at ..\win-x64\AppX\IpodSync.UI.exe), so we
        //    also check the parent dir. Documented WinUI dev quirk.
        var uiExeDir = GetUiExeDirectory();
        if (uiExeDir is not null)
        {
            var sibling = Path.GetFullPath(Path.Combine(uiExeDir, CoreExeName));
            tried.Add(sibling);
            if (File.Exists(sibling)) return sibling;

            var parent = Directory.GetParent(uiExeDir);
            if (parent is not null)
            {
                var parentSibling = Path.GetFullPath(Path.Combine(parent.FullName, CoreExeName));
                tried.Add(parentSibling);
                if (File.Exists(parentSibling)) return parentSibling;
            }
        }

        // 3. PATH lookup.
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
        lines.Add("  - From the repo root, run `cargo build --release` to produce");
        lines.Add("    target\\release\\ipod-sync.exe, THEN rebuild the UI");
        lines.Add("    (`dotnet build` will copy the core binary next to the UI exe).");
        lines.Add("  - If you've installed an MSIX, the core binary should be packaged");
        lines.Add("    alongside the UI exe; re-install the latest release.");
        lines.Add("  - Or add the build's containing directory to your PATH.");
        return string.Join('\n', lines);
    }
}

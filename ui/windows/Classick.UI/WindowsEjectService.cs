using System.Diagnostics;
using Classick_UI.Devices;

namespace Classick_UI;

internal static class WindowsEjectService
{
    public static async Task EjectAsync(DeviceMountTarget target)
    {
        var root = Path.GetPathRoot(target.MountPath);
        if (string.IsNullOrWhiteSpace(root) || root.Length < 2 || root[1] != ':')
            throw new InvalidOperationException("The selected iPod does not have a Windows drive mount.");

        var start = new ProcessStartInfo
        {
            FileName = "powershell.exe",
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        start.ArgumentList.Add("-NoProfile");
        start.ArgumentList.Add("-NonInteractive");
        start.ArgumentList.Add("-WindowStyle");
        start.ArgumentList.Add("Hidden");
        start.ArgumentList.Add("-Command");
        start.ArgumentList.Add("(New-Object -ComObject Shell.Application).NameSpace(17).ParseName($args[0]).InvokeVerb('Eject')");
        start.ArgumentList.Add(root[..2]);

        using var process = Process.Start(start) ??
            throw new InvalidOperationException("Windows could not start the eject request.");
        await process.WaitForExitAsync();
        if (process.ExitCode != 0)
            throw new InvalidOperationException("Windows did not accept the eject request.");
    }
}

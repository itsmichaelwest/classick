using Classick_UI.Ipc;

namespace Classick_UI.Devices;

public enum DeviceArtworkSpecificity
{
    Generic,
    Exact,
}

public sealed record DeviceArtwork(
    string AssetUri,
    string AccessibleDescription,
    DeviceArtworkSpecificity Specificity);

public sealed record DevicePresentation(
    string Title,
    string HardwareSummary,
    string HardwareProvenance,
    string Status,
    string Guidance,
    bool CanAdopt,
    DeviceArtwork Artwork);

public static class DevicePresentationFactory
{
    private const string GenericIpodAsset = "ms-appx:///Assets/ipod-generic.svg";

    private static readonly IReadOnlyDictionary<(string ModelCode, IpodColour Colour), string> ExactArtwork =
        new Dictionary<(string, IpodColour), string>
        {
            [("MC293", IpodColour.Silver)] = "ms-appx:///Assets/ipod.svg",
        };

    public static DevicePresentation For(IdentifiedDeviceSnapshot device)
    {
        ArgumentNullException.ThrowIfNull(device);
        var family = FamilyLabel(device.Hardware.Family?.Value);
        var title = !string.IsNullOrWhiteSpace(device.Name)
            ? device.Name!
            : device.Hardware.Generation?.Value ?? family;
        var hardware = HardwareSummary(device.Hardware, family);
        var canAdopt = device is
        {
            Connected: true,
            Readiness: DeviceReadiness.Ready,
            ProfileStatus: ProfileStatus.NotAdopted,
        };
        var (status, guidance) = Status(device, canAdopt);
        return new DevicePresentation(
            title,
            hardware,
            HardwareProvenance(device.Hardware),
            status,
            guidance,
            canAdopt,
            Artwork(device.Hardware, family));
    }

    public static DevicePresentation For(UnidentifiedDeviceSnapshot device)
    {
        ArgumentNullException.ThrowIfNull(device);
        var family = FamilyLabel(device.Hardware.Family?.Value);
        return new DevicePresentation(
            family,
            HardwareSummary(device.Hardware, family),
            HardwareProvenance(device.Hardware),
            "Identity unavailable",
            "Classick cannot safely target this connection. Reconnect the iPod and try again; administrator access is not required for ordinary sync.",
            false,
            GenericArtwork(family));
    }

    private static (string Status, string Guidance) Status(IdentifiedDeviceSnapshot device, bool canAdopt)
    {
        var result = device.Readiness switch
        {
            DeviceReadiness.NeedsAppleInitialization => (
                "Apple setup required",
                "Set up this iPod in Finder, Apple Devices, or iTunes first. Classick does not initialize iPods."),
            DeviceReadiness.InvalidDatabase => (
                "iPod database is invalid",
                "Restore or repair the iPod with Apple software before using Classick."),
            DeviceReadiness.IdentityUnavailable => (
                "Identity unavailable",
                "Classick cannot safely target this connection. Reconnect the iPod and try again."),
            DeviceReadiness.Ready when device.ProfileStatus == ProfileStatus.RecoveryRequired => (
                "Classick recovery required",
                "Finish or recover the previous Classick operation before changing this iPod."),
            DeviceReadiness.Ready when device.ProfileStatus == ProfileStatus.Invalid => (
                "Classick profile is invalid",
                "Remove the invalid Classick profile before setting up this iPod again."),
            DeviceReadiness.Ready when device.ProfileStatus == ProfileStatus.Adopted => (
                "Ready · Classick profile adopted",
                ""),
            DeviceReadiness.Ready when device.ProfileStatus == ProfileStatus.PendingAdoption => (
                "Classick setup pending",
                "Classick has accepted setup and will finish when the iPod is available."),
            DeviceReadiness.Ready when canAdopt => (
                "Ready for Classick",
                "Apple software has initialized this iPod and Classick can set up its sync profile."),
            _ => ("Unavailable", "This iPod cannot be set up right now."),
        };
        if (device.Connected) return result;
        return device.Readiness == DeviceReadiness.Ready
            ? ("Not connected", "Reconnect this iPod to use it with Classick.")
            : ($"{result.Item1} · not connected", $"{result.Item2} Reconnect the iPod after completing those steps.");
    }

    private static DeviceArtwork Artwork(HardwareFacts facts, string family)
    {
        if (CertainNonInferred(facts.ModelCode) && CertainNonInferred(facts.Colour) &&
            ExactArtwork.TryGetValue((facts.ModelCode!.Value.ToUpperInvariant(), facts.Colour!.Value), out var asset))
        {
            return new DeviceArtwork(
                asset,
                $"{ColourLabel(facts.Colour.Value)} {family}",
                DeviceArtworkSpecificity.Exact);
        }
        return GenericArtwork(family);
    }

    private static DeviceArtwork GenericArtwork(string family) =>
        new(GenericIpodAsset, family, DeviceArtworkSpecificity.Generic);

    private static bool CertainNonInferred<T>(HardwareFact<T>? fact) =>
        fact is { Confidence: FactConfidence.Certain } && fact.Source != FactSource.Inferred;

    private static string HardwareSummary(HardwareFacts facts, string family)
    {
        var parts = new List<string>();
        if (!string.IsNullOrWhiteSpace(facts.Generation?.Value)) parts.Add(facts.Generation.Value);
        if (!parts.Contains(family, StringComparer.OrdinalIgnoreCase)) parts.Add(family);
        if (!string.IsNullOrWhiteSpace(facts.ModelCode?.Value)) parts.Add(facts.ModelCode.Value.ToUpperInvariant());
        if (facts.Colour is { } colour) parts.Add(ColourLabel(colour.Value));
        return string.Join(" · ", parts);
    }

    private static string HardwareProvenance(HardwareFacts facts)
    {
        var parts = new List<string>();
        AddProvenance(parts, "Family", facts.Family);
        AddProvenance(parts, "Generation", facts.Generation);
        AddProvenance(parts, "Model", facts.ModelCode);
        AddProvenance(parts, "Colour", facts.Colour);
        AddProvenance(parts, "Firmware", facts.Firmware);
        AddProvenance(parts, "Capacity", facts.CapacityBytes);
        return string.Join(" · ", parts);
    }

    private static void AddProvenance<T>(List<string> parts, string label, HardwareFact<T>? fact)
    {
        if (fact is null) return;
        parts.Add($"{label}: {fact.Source.ToString().ToLowerInvariant()}, {fact.Confidence.ToString().ToLowerInvariant()}");
    }

    private static string FamilyLabel(IpodFamily? family) => family switch
    {
        IpodFamily.Classic => "iPod classic",
        IpodFamily.Nano => "iPod nano",
        IpodFamily.Mini => "iPod mini",
        IpodFamily.Shuffle => "iPod shuffle",
        IpodFamily.Photo => "iPod photo",
        IpodFamily.Video => "iPod video",
        IpodFamily.Touch => "iPod touch",
        _ => "iPod",
    };

    private static string ColourLabel(IpodColour colour) => colour switch
    {
        IpodColour.StainlessSteel => "stainless steel",
        _ => colour.ToString().ToLowerInvariant(),
    };
}

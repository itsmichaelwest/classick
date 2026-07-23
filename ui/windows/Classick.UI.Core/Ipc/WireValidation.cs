using System.Text.Json;

namespace Classick_UI.Ipc;

internal static partial class WireValidation
{
    public static void Validate(WireMessage message)
    {
        switch (message)
        {
            case WireHello hello:
                WireCodec.ValidateHello(hello);
                return;
            case WireCommand command:
                ValidateDeviceId(command);
                ValidateRequestId(command);
                ValidateCommand(command);
                return;
            case WireEvent wireEvent:
                ValidateDeviceId(wireEvent);
                ValidateRequestId(wireEvent);
                ValidateEvent(wireEvent);
                return;
            default:
                throw new JsonException("unknown wire message");
        }
    }

    private static void ValidateRequestId(WireMessage message)
    {
        var property = message.GetType().GetProperty("RequestId");
        if (property is null) return;
        if (property.GetValue(message) is string requestId)
        {
            WireCodec.ValidateUuid(requestId, "request ID");
            return;
        }
        if (message is not GlobalConfigEvent and not WireSourceAvailabilityEvent and not DeviceInventoryEvent and
            not DeviceConfigEvent and not HistoryEvent and not LibraryEvent and not PlaylistsEvent and
            not LibraryScanStartedEvent and not LibraryScanProgressEvent and not LibraryScanFinishedEvent)
            throw new JsonException("request ID must not be null");
    }

    private static void ValidateDeviceId(WireMessage message)
    {
        var property = message.GetType().GetProperty("DeviceId");
        if (property is not null && property.GetValue(message) is not DeviceId)
            throw new JsonException("device ID must not be null");
    }

    private static void ValidateCommand(WireCommand command)
    {
        if (command is ISessionRoutedMessage routed)
        {
            RequireNonzero(routed.SessionId, "session ID");
        }
        switch (command)
        {
            case SetSourceLocationCommand { SourceRoot: { } root }:
                ValidateSourceRoot(root);
                break;
            case AdoptDeviceCommand adopt:
                ValidateMutationId(adopt.SelectionMutationId);
                ValidateMutationId(adopt.SettingsMutationId);
                ValidateMutationId(adopt.SubscriptionsMutationId);
                if (new HashSet<string>([adopt.SelectionMutationId, adopt.SettingsMutationId, adopt.SubscriptionsMutationId], StringComparer.Ordinal).Count != 3)
                {
                    throw new JsonException("adoption mutation IDs must be unique");
                }
                ValidateSelection(adopt.Selection);
                ValidateSettings(adopt.Settings);
                ValidateSubscriptions(adopt.Subscriptions);
                break;
            case SetSelectionCommand selection:
                ValidateMutationId(selection.MutationId);
                ValidateSelection(selection.Selection);
                break;
            case SetSettingsCommand settings:
                ValidateMutationId(settings.MutationId);
                ValidateSettings(settings.Settings);
                break;
            case SetSubscriptionsCommand subscriptions:
                ValidateMutationId(subscriptions.MutationId);
                ValidateSubscriptions(subscriptions.Subscriptions);
                break;
            case WireGetHistoryCommand history when history.Limit is 0 or > 50:
                throw new JsonException("history limit must be between 1 and 50");
            case PreviewSelectionCommand preview:
                ValidateSelection(preview.Selection);
                break;
            case ResolveTracksCommand resolve:
                ValidateSelectionRules(resolve.Rules, requireNonempty: false);
                break;
            case AddSelectionToDeviceCommand mutation:
                ValidateMutationId(mutation.MutationId);
                ValidateSelectionRules(mutation.Rules, requireNonempty: true);
                break;
            case AppendSelectionToPlaylistCommand append:
                ValidateSlug(append.Slug);
                ValidateSelectionRules(append.Rules, requireNonempty: true);
                break;
            case GetPlaylistCommand get:
                ValidateSlug(get.Slug);
                break;
            case DeletePlaylistCommand delete:
                ValidateSlug(delete.Slug);
                break;
            case SavePlaylistCommand save:
                ValidatePlaylist(save.Playlist, stored: false);
                break;
            case PromptDecisionCommand prompt:
                RequireNonzero(prompt.PromptId, "prompt ID");
                break;
            case FormDecisionCommand form:
                RequireNonzero(form.PromptId, "prompt ID");
                break;
        }
    }

    private static void ValidateEvent(WireEvent wireEvent)
    {
        if (wireEvent is ISessionRoutedMessage routed)
        {
            RequireNonzero(routed.SessionId, "session ID");
        }
        switch (wireEvent)
        {
            case GlobalConfigEvent global:
                RequireNonzero(global.Revision, "global config revision");
                if (global.SourceRoot is not null) ValidateSourceRoot(global.SourceRoot);
                break;
            case WireSourceAvailabilityEvent source:
                if ((source.State == SourceAvailabilityState.Available) != (source.SourceRoot is not null))
                {
                    throw new JsonException("available source requires a root and unavailable source must omit it");
                }
                if (source.SourceRoot is not null) ValidateSourceRoot(source.SourceRoot);
                break;
            case DeviceInventoryEvent inventory:
                ValidateInventory(inventory);
                break;
            case DeviceConfigEvent config:
                ValidateDeviceConfig(config.Selection, config.Settings, config.Subscriptions);
                break;
            case ConfigMutationFailedEvent failure:
                ValidateMutationId(failure.MutationId);
                RequireText(failure.Message, "configuration mutation failure message");
                break;
            case WireSyncRejectedEvent rejected:
                RequireText(rejected.Message, "sync rejection message");
                break;
            case HistoryEvent history:
                foreach (var entry in history.Entries) ValidateHistory(entry);
                break;
            case LibraryEvent library:
                ValidateLibrary(library);
                break;
            case LibraryScanStartedEvent scan:
                RequireNonzero(scan.SessionId, "scan session ID");
                break;
            case LibraryScanProgressEvent progress:
                RequireNonzero(progress.SessionId, "scan session ID");
                if (progress.TracksIndexed > progress.FilesScanned)
                    throw new JsonException("library scan cannot index more tracks than files scanned");
                break;
            case LibraryScanFinishedEvent finished:
                RequireNonzero(finished.SessionId, "scan session ID");
                if ((finished.Success && finished.Message is not null) ||
                    (!finished.Success && string.IsNullOrEmpty(finished.Message)))
                    throw new JsonException("library scan result has inconsistent diagnostics");
                break;
            case ResolvedTracksEvent resolved:
                ValidateSortedUnique(resolved.Tracks, "resolved tracks");
                foreach (var path in resolved.Tracks) ValidateLibraryPath(path);
                break;
            case DevicePreviewEvent preview:
                ValidateSortedUnique(preview.UnresolvedSubscriptions, "unresolved subscriptions");
                foreach (var slug in preview.UnresolvedSubscriptions) ValidateSlug(slug);
                break;
            case PlaylistsEvent playlists:
                RequireNonzero(playlists.Revision, "playlist collection revision");
                ValidateSortedUnique(playlists.Playlists.Select(item => item.Slug), "playlist collection");
                foreach (var playlist in playlists.Playlists) ValidatePlaylistSummary(playlist);
                break;
            case PlaylistDetailEvent detail:
                RequireNonzero(detail.Revision, "playlist detail revision");
                ValidateSlug(detail.Slug);
                ValidatePlaylistDetail(detail);
                break;
            case PlaylistSavedEvent saved:
                RequireNonzero(saved.Revision, "saved playlist revision");
                ValidatePlaylist(saved.Playlist, stored: true);
                break;
            case DeviceSelectionAddedEvent added:
                ValidateMutationId(added.MutationId);
                RequireNonzero(added.SelectionRevision, "device selection revision");
                ValidateSelection(added.Selection);
                ValidateDelivery(added.Delivery);
                if (added.Sync is StartedSyncDisposition started)
                {
                    RequireNonzero(started.SessionId, "started sync session ID");
                    if (added.Delivery is not DeviceCommittedDelivery)
                        throw new JsonException("started sync requires committed delivery");
                }
                break;
            case PlaylistSelectionAppendedEvent appended:
                RequireNonzero(appended.Revision, "playlist append revision");
                ValidateSlug(appended.Slug);
                ValidatePlaylist(appended.Playlist, stored: true);
                if (appended.Playlist is not ManualPlaylist manual || manual.Slug != appended.Slug)
                    throw new JsonException("playlist append requires its matching manual playlist");
                break;
            case LibraryMutationRejectedEvent rejected:
                RequireText(rejected.Code, "library mutation rejection code");
                RequireText(rejected.Message, "library mutation rejection message");
                ValidateMutationTarget(rejected.Target);
                break;
            case RunHeaderEvent header:
                RequireText(header.Source, "source path");
                RequireText(header.Ipod, "iPod path");
                RequireText(header.Manifest, "manifest path");
                break;
            case SyncSummaryEvent summary:
                ValidateActionPlan(summary.Summary);
                break;
            case ReviewRequestedEvent review:
                ValidateActionPlan(review.Summary);
                break;
            case WirePromptEvent prompt:
                RequireNonzero(prompt.PromptId, "prompt ID");
                RequireText(prompt.Message, "prompt message");
                if (prompt.Options.Count == 0 || prompt.Options.Any(string.IsNullOrEmpty))
                    throw new JsonException("prompt requires non-empty options");
                break;
            case WireFormEvent form:
                RequireNonzero(form.PromptId, "prompt ID");
                RequireText(form.Label, "form label");
                break;
            case WireTrackStartEvent track when track.Total == 0 || track.Current == 0 || track.Current > track.Total || string.IsNullOrEmpty(track.Label):
                throw new JsonException("track start requires a valid 1-based position and label");
            case SyncLogEvent log:
                RequireText(log.Message, "sync log message");
                break;
            case SyncErrorEvent error:
                RequireText(error.Message, "sync error message");
                break;
            case SyncFinishedEvent finished:
                ValidateFinish(finished);
                break;
            case CommandFailedEvent failed:
                RequireText(failed.Message, "command failure message");
                break;
        }
    }

    private static void ValidateInventory(DeviceInventoryEvent inventory)
    {
        RequireNonzero(inventory.Revision, "inventory revision");
        var deviceIds = new HashSet<DeviceId>();
        var mounts = new HashSet<string>(StringComparer.Ordinal);
        foreach (var device in inventory.Devices)
        {
            if (!deviceIds.Add(device.DeviceId)) throw new JsonException("inventory repeats a device");
            if (device.Name is "") throw new JsonException("device name must not be empty");
            ValidateHardware(device.Hardware);
            if (device.Readiness == DeviceReadiness.IdentityUnavailable)
                throw new JsonException("identified device cannot be identity unavailable");
            if (device.Storage is { } storage && (storage.TotalBytes == 0 || storage.FreeBytes > storage.TotalBytes))
                throw new JsonException("device storage snapshot is inconsistent");
            if (device.Connected)
            {
                if (device.MountPath is null || !IsAbsoluteNativePath(device.MountPath))
                    throw new JsonException("connected device requires an absolute mount path");
                if (!mounts.Add(device.MountPath)) throw new JsonException("inventory repeats a mount path");
                if (device.Phase == DevicePhase.Disconnected) throw new JsonException("connected device cannot be disconnected");
            }
            else
            {
                if (device.MountPath is not null || device.SessionId is not null || device.Phase != DevicePhase.Disconnected)
                    throw new JsonException("disconnected device retains connected-only state");
                if (device.Storage is { Freshness: not StorageFreshness.Cached })
                    throw new JsonException("disconnected storage must be cached");
            }
            if (device.Phase == DevicePhase.Syncing)
            {
                if (device.SessionId is null || device.Readiness != DeviceReadiness.Ready || device.ProfileStatus != ProfileStatus.Adopted)
                    throw new JsonException("syncing device must be ready, adopted, and routed");
                RequireNonzero(device.SessionId.Value, "inventory session ID");
            }
            else if (device.SessionId is not null)
            {
                throw new JsonException("session ID is only valid for syncing devices");
            }
        }

        var observations = new HashSet<ulong>();
        foreach (var device in inventory.Unidentified)
        {
            RequireNonzero(device.ObservationId, "observation ID");
            if (!observations.Add(device.ObservationId)) throw new JsonException("inventory repeats an observation");
            if (device.Readiness != DeviceReadiness.IdentityUnavailable)
                throw new JsonException("unidentified device must be identity unavailable");
            ValidateHardware(device.Hardware);
        }
    }

    private static void ValidateHardware(HardwareFacts hardware)
    {
        ValidateFact(hardware.Family);
        ValidateFact(hardware.Colour);
        ValidateFact(hardware.CapacityBytes);
        ValidateTextFact(hardware.Generation);
        ValidateTextFact(hardware.ModelCode);
        ValidateTextFact(hardware.Firmware);
        if (hardware.CapacityBytes is { Value: 0 }) throw new JsonException("hardware capacity must be nonzero");
    }

    private static void ValidateFact<T>(HardwareFact<T>? fact)
    {
        if (fact is null) return;
        var valid = fact.Source switch
        {
            FactSource.Reported or FactSource.Decoded => fact.Confidence == FactConfidence.Certain,
            FactSource.Inferred => fact.Confidence == FactConfidence.Heuristic,
            _ => false,
        };
        if (!valid) throw new JsonException("hardware fact has inconsistent provenance");
    }

    private static void ValidateTextFact(HardwareFact<string>? fact)
    {
        ValidateFact(fact);
        if (fact is { Value: "" }) throw new JsonException("hardware text fact must not be empty");
    }

    private static void ValidateDeviceConfig(
        DeliveredComponent<SelectionValue> selection,
        DeliveredComponent<SettingsValue> settings,
        DeliveredComponent<SubscriptionsValue> subscriptions)
    {
        RequireNonzero(selection.Revision, "selection revision");
        RequireNonzero(settings.Revision, "settings revision");
        RequireNonzero(subscriptions.Revision, "subscriptions revision");
        ValidateMutationId(selection.MutationId);
        ValidateMutationId(settings.MutationId);
        ValidateMutationId(subscriptions.MutationId);
        if (new HashSet<string>([selection.MutationId, settings.MutationId, subscriptions.MutationId], StringComparer.Ordinal).Count != 3)
            throw new JsonException("device config repeats a mutation ID");
        ValidateSelection(selection.Value);
        ValidateSettings(settings.Value);
        ValidateSubscriptions(subscriptions.Value);
        ValidateDelivery(selection.Delivery);
        ValidateDelivery(settings.Delivery);
        ValidateDelivery(subscriptions.Delivery);
    }

    private static void ValidateDelivery(ConfigDelivery delivery)
    {
        if (delivery is PendingDeviceDelivery { LastFailure: "" })
            throw new JsonException("pending delivery failure requires a message");
    }

    private static void ValidateSelection(SelectionValue selection)
    {
        if (selection.SchemaVersion != 1) throw new JsonException("unsupported selection schema");
        ValidateSelectionRules(selection.Rules, requireNonempty: false);
    }

    private static void ValidateSettings(SettingsValue settings)
    {
        if (settings.SchemaVersion != 1) throw new JsonException("unsupported settings schema");
    }

    private static void ValidateSubscriptions(SubscriptionsValue subscriptions)
    {
        if (subscriptions.SchemaVersion != 1) throw new JsonException("unsupported subscriptions schema");
        foreach (var slug in subscriptions.Playlists) ValidateSlug(slug);
        if (subscriptions.Playlists.Distinct(StringComparer.Ordinal).Count() != subscriptions.Playlists.Count)
            throw new JsonException("subscriptions repeat a playlist");
    }

    private static void ValidateSelectionRules(IReadOnlyList<SelectionRule> rules, bool requireNonempty)
    {
        if (requireNonempty && rules.Count == 0) throw new JsonException("library mutation requires rules");
        foreach (var rule in rules)
        {
            if (rule is ArtistSelectionRule { Name: "" } or GenreSelectionRule { Name: "" } ||
                rule is AlbumSelectionRule { Artist: "" } or AlbumSelectionRule { Album: "" })
                throw new JsonException("selection rule labels must not be empty");
        }
    }

    private static void ValidateSourceRoot(string root)
    {
        if (string.IsNullOrEmpty(root) || root.Any(char.IsControl)) throw new JsonException("source root is invalid");
        if (root.StartsWith("smb://", StringComparison.OrdinalIgnoreCase))
        {
            var remainder = root[6..];
            var slash = remainder.IndexOf('/');
            if (slash <= 0 || slash == remainder.Length - 1 || remainder[..slash].Contains('@') ||
                root.Contains('?') || root.Contains('#'))
                throw new JsonException("SMB source must be credential-free and include a share");
        }
        else if (!IsAbsoluteNativePath(root))
        {
            throw new JsonException("source root must be absolute");
        }
    }

    private static bool IsAbsoluteNativePath(string path) =>
        !string.IsNullOrEmpty(path) && !path.Contains('\0') &&
        (path.StartsWith('/') || path.StartsWith("\\\\", StringComparison.Ordinal) ||
         (path.Length >= 3 && char.IsAsciiLetter(path[0]) && path[1] == ':' && path[2] is '\\' or '/'));

    private static void ValidateSlug(string value)
    {
        if (string.IsNullOrEmpty(value) || !char.IsAsciiLetterOrDigit(value[0]) || !char.IsAsciiLetterOrDigit(value[^1]) ||
            value.Any(character => !((character is >= 'a' and <= 'z') || char.IsAsciiDigit(character) || character == '-')) ||
            value.Contains("--", StringComparison.Ordinal))
            throw new JsonException("playlist slug is invalid");
    }

    private static void ValidateProfilePath(string value)
    {
        if (string.IsNullOrEmpty(value) || value.StartsWith('/') || value.EndsWith('/') || value.Contains("//", StringComparison.Ordinal) ||
            value.Any(character => !char.IsAscii(character) || character is ':' or '*' or '?' or '"' or '<' or '>' or '|' or '@' || char.IsControl(character)))
            throw new JsonException("profile path is invalid");
    }

    private static void ValidateLibraryPath(string value)
    {
        if (string.IsNullOrEmpty(value) || value.StartsWith('/') || value.Contains('\\') ||
            (value.Length > 1 && char.IsAsciiLetter(value[0]) && value[1] == ':') ||
            value.Split('/').Any(component => component is "" or "." or ".."))
            throw new JsonException("library path is invalid");
    }

    private static void ValidateSortedUnique(IEnumerable<string> values, string label)
    {
        string? previous = null;
        foreach (var value in values)
        {
            if (previous is not null && string.CompareOrdinal(previous, value) >= 0)
                throw new JsonException($"{label} must be sorted and unique");
            previous = value;
        }
    }

    private static void ValidateMutationId(string value) => WireCodec.ValidateUuid(value, "mutation ID");
    private static void RequireNonzero(ulong value, string label)
    {
        if (value == 0) throw new JsonException($"{label} must be nonzero");
    }
    private static void RequireText(string value, string label)
    {
        if (string.IsNullOrEmpty(value)) throw new JsonException($"{label} must not be empty");
    }
    private static void RequireSafeText(string value, string label)
    {
        if (string.IsNullOrEmpty(value) || value.Any(char.IsControl)) throw new JsonException($"{label} is invalid");
    }
}

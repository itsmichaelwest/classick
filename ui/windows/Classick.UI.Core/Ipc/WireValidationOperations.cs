using System.Text.Json;

namespace Classick_UI.Ipc;

internal static partial class WireValidation
{
    private static void ValidateLibrary(LibraryEvent library)
    {
        var hasContent = library.ScannedAtUnixSecs is not null || library.TotalTracks != 0 || library.TotalBytes != 0 ||
            library.Artists.Count != 0 || library.Genres.Count != 0;
        if ((library.SourceRoot is null || library.ScannedAtUnixSecs is null) && hasContent)
            throw new JsonException("unconfigured or unscanned library cannot contain content");
        if (library.SourceRoot is not null) ValidateSourceRoot(library.SourceRoot);
        foreach (var artist in library.Artists)
        {
            RequireSafeText(artist.Name, "library artist");
            foreach (var album in artist.Albums)
            {
                RequireSafeText(album.Name, "library album");
                if (album.Genre is not null) RequireSafeText(album.Genre, "library album genre");
            }
        }
        foreach (var genre in library.Genres) RequireSafeText(genre.Name, "library genre");
    }

    private static void ValidateHistory(WireHistoryEntry entry)
    {
        if (entry.SessionId is { } sessionId) RequireNonzero(sessionId, "history session ID");
        RequireSafeText(entry.Timestamp, "history timestamp");
        if (entry.ErrorMessage is "") throw new JsonException("history error must not be empty");
        if (entry.Outcome == SyncOutcome.Ok && entry.ErrorMessage is not null)
            throw new JsonException("successful history entry cannot carry an error");
    }

    private static void ValidatePlaylistSummary(PlaylistSummary playlist)
    {
        ValidateSlug(playlist.Slug);
        RequireText(playlist.Name, "playlist name");
        if (playlist.Error is "" || (playlist.Error is not null && (playlist.Tracks != 0 || playlist.Bytes != 0)))
            throw new JsonException("playlist summary contains inconsistent content");
    }

    private static void ValidatePlaylistDetail(PlaylistDetailEvent detail)
    {
        switch (detail.Result)
        {
            case FoundPlaylistDetail found:
                ValidatePlaylist(found.Playlist, stored: true);
                if (ReadPlaylistSlug(found.Playlist) != detail.Slug)
                    throw new JsonException("playlist detail slug mismatch");
                break;
            case UnavailablePlaylistDetail unavailable:
                RequireText(unavailable.Message, "playlist unavailable message");
                break;
        }
    }

    private static void ValidatePlaylist(Playlist playlist, bool stored)
    {
        RequireSafeText(playlist switch { ManualPlaylist manual => manual.Name, SmartPlaylist smart => smart.Name, _ => "" }, "playlist name");
        var slug = ReadPlaylistSlug(playlist);
        if (stored && slug is null) throw new JsonException("stored playlist requires a slug");
        if (slug is not null) ValidateSlug(slug);
        switch (playlist)
        {
            case ManualPlaylist manual:
                foreach (var track in manual.Tracks) ValidateProfilePath(track);
                break;
            case SmartPlaylist smart:
                ValidateSmartRules(smart.Rules);
                break;
        }
    }

    private static string? ReadPlaylistSlug(Playlist playlist) =>
        playlist switch { ManualPlaylist manual => manual.Slug, SmartPlaylist smart => smart.Slug, _ => null };

    private static void ValidateSmartRules(SmartRules rules)
    {
        if (rules.Version != 1) throw new JsonException("unsupported smart playlist rule version");
        foreach (var rule in rules.Rules) RequireSafeText(rule.Value, "smart playlist rule");
        if (rules.Limit is TrackSmartLimit { Tracks: 0 } or ByteSmartLimit { Bytes: 0 })
            throw new JsonException("smart playlist limit must be nonzero");
    }

    private static void ValidateMutationTarget(LibraryMutationTarget target)
    {
        if (target is ManualPlaylistMutationTarget playlist) ValidateSlug(playlist.Slug);
    }

    private static void ValidateActionPlan(WireActionPlanSummary summary)
    {
        try
        {
            var withoutRemovals = checked(summary.Add + summary.Modify + summary.MetadataOnly);
            var withRemovals = checked(withoutRemovals + summary.Remove);
            if (summary.TotalPlanned != withoutRemovals && summary.TotalPlanned != withRemovals)
                throw new JsonException("action plan total does not match its counts");
        }
        catch (OverflowException exception)
        {
            throw new JsonException("action plan count overflow", exception);
        }
    }

    private static void ValidateFinish(SyncFinishedEvent finished)
    {
        if (finished.SkippedForSpace is { } skipped && (skipped.Albums == 0 || skipped.Tracks == 0 || skipped.Bytes == 0))
            throw new JsonException("skipped-for-space summary must be nonzero");
        if (finished.Artwork is { } artwork &&
            (artwork.Embedded > artwork.Eligible || artwork.FailedSources > artwork.Eligible ||
             artwork.Embedded > artwork.Eligible - artwork.FailedSources))
            throw new JsonException("artwork summary counts are inconsistent");
    }
}

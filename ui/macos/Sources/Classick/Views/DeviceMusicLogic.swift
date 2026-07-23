enum DeviceMusicLogic {
    enum MusicPageContentState: Equatable {
        case needsScan
        case scanning(current: Int, total: Int)
        case libraryEmpty(path: String)
        case deviceEmpty
        case browser
    }

    struct CapacityBar: Equatable {
        var usedFraction: Double
        var projectedFraction: Double
        var usedBytes: UInt64
        var projectedBytes: UInt64
        var totalBytes: UInt64
    }

    static func caption(mode: SelectionMode, isConnected: Bool) -> String {
        guard isConnected else { return "Not connected — changes apply on next sync" }
        switch mode {
        case .all: return "Everything in your library syncs to this iPod."
        case .include: return "Checked items sync to this iPod."
        case .exclude: return "Checked items are left off this iPod."
        }
    }

    static func contentState(
        library: LibraryInfo?, phase: Phase, configuredSource: String?,
        mode: SelectionMode, isConnected: Bool, syncedCount: Int
    ) -> MusicPageContentState {
        switch LibraryContentLogic.state(
            library: library, phase: phase, configuredSource: configuredSource
        ) {
        case .needsScan: return .needsScan
        case let .scanning(current, total): return .scanning(current: current, total: total)
        case let .libraryEmpty(path): return .libraryEmpty(path: path)
        case .browse:
            guard mode == .all, isConnected, syncedCount == 0 else { return .browser }
            return .deviceEmpty
        }
    }

    static func isSyncNowDisabled(phase: Phase, isConnected: Bool) -> Bool {
        guard isConnected else { return true }
        switch phase {
        case .syncing, .scanning, .noDevice: return true
        default: return false
        }
    }

    /// Preserves remembered per-mode rules and makes a first Entire-to-
    /// Selected transition zero-diff by snapshotting the known albums.
    static func seededSelection(
        fromDeviceContents artists: [LibraryArtist],
        previousMode: SelectionMode,
        newMode: SelectionMode,
        current: [SelectionRule],
        remembered: [SelectionRule]? = nil
    ) -> [SelectionRule] {
        guard previousMode != newMode else { return current }
        switch (previousMode, newMode) {
        case (.all, .include):
            if let remembered { return remembered }
            return artists.flatMap { artist in
                artist.albums.map { SelectionRule.album(artist: artist.name, album: $0.name) }
            }
        case (.all, .exclude):
            return remembered ?? []
        case (.include, .exclude), (.exclude, .include):
            return remembered ?? current
        case (_, .all):
            return current
        default:
            return current
        }
    }

    static func capacityBar(
        storage: (free: Int64, total: Int64)?,
        preview: DevicePreview?
    ) -> CapacityBar? {
        guard let storage, storage.total > 0, let preview else { return nil }
        let total = UInt64(storage.total)
        let used = preview.selectedBytes + preview.playlistExtraBytes
        let projectedFree = preview.projectedFreeBytes ?? (total > used ? total - used : 0)
        let projectedUsed = total > projectedFree ? total - projectedFree : total
        return CapacityBar(
            usedFraction: min(1, Double(used) / Double(total)),
            projectedFraction: min(1, Double(projectedUsed) / Double(total)),
            usedBytes: used, projectedBytes: projectedUsed, totalBytes: total)
    }

    static func capacitySummary(_ bar: CapacityBar) -> String {
        "\(DeviceRowFormatting.gbString(bar.usedBytes)) of \(DeviceRowFormatting.gbString(bar.totalBytes)) used"
    }

    static func unresolvedSubscriptionsLine(_ unresolved: [String]?) -> String? {
        guard let unresolved, !unresolved.isEmpty else { return nil }
        return "\(unresolved.count) subscribed playlist\(unresolved.count == 1 ? "" : "s") couldn't be resolved"
    }

    static func scrubbedSubscriptions(
        _ subscriptions: Set<String>, validSlugs: Set<String>
    ) -> Set<String> {
        subscriptions.intersection(validSlugs)
    }
}

enum DeviceSurfaceLogic {
    static func state(
        serial: DeviceID, in devices: [DeviceID: DeviceViewState]
    ) -> DeviceViewState? {
        devices[serial]
    }

    static func phase(for state: DeviceViewState?, globalPhase: Phase) -> Phase {
        if case .scanning = globalPhase { return globalPhase }
        guard let state else { return .noDevice }
        switch state.phase {
        case .disconnected: return .noDevice
        case .unconfigured: return .notConfigured
        case .idle: return .idle
        case .syncing:
            let progress = state.syncProgress
            return .syncing(
                current: progress?.current ?? 0,
                total: progress?.total ?? 0,
                label: progress?.label ?? "",
                etaSecs: progress?.etaSecs)
        case .paused: return .paused(synced: state.syncedCount, total: state.libraryCount)
        case .error(let message): return .error(message)
        }
    }

    static func storage(_ state: DeviceViewState?) -> (free: Int64, total: Int64)? {
        guard let storage = state?.storage else { return nil }
        return (Int64(clamping: storage.free), Int64(clamping: storage.total))
    }

    static func storageText(_ state: DeviceViewState?) -> String? {
        guard let storage = state?.storage else { return nil }
        return "\(storage.free / 1_000_000_000) / \(storage.total / 1_000_000_000) GB"
    }
}

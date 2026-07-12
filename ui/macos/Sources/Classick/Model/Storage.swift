import Foundation

// `status_update.storage` is always nil on the macOS wire (see Global
// Constraints in the app plan) — compute free/total capacity app-side from
// the iPod's mounted volume path instead of trusting the daemon.
func storageFor(drive: String) -> (free: Int64, total: Int64)? {
    let url = URL(fileURLWithPath: drive)
    guard let v = try? url.resourceValues(forKeys: [.volumeAvailableCapacityKey, .volumeTotalCapacityKey]),
          let free = v.volumeAvailableCapacity, let total = v.volumeTotalCapacity else { return nil }
    return (Int64(free), Int64(total))
}

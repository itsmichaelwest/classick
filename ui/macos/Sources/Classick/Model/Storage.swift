import Foundation

// Mounted-volume fallback used when a status snapshot has no storage value.
func storageFor(drive: String) -> (free: Int64, total: Int64)? {
    let url = URL(fileURLWithPath: drive)
    guard let v = try? url.resourceValues(forKeys: [.volumeAvailableCapacityKey, .volumeTotalCapacityKey]),
          let free = v.volumeAvailableCapacity, let total = v.volumeTotalCapacity else { return nil }
    return (Int64(free), Int64(total))
}

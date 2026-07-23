import Darwin
import Foundation

enum DaemonSocketIO {
  static func lines(from descriptor: Int32) -> AsyncStream<Data> {
    AsyncStream { continuation in
      let thread = Thread {
        var buffer = Data()
        var readBuffer = [UInt8](repeating: 0, count: 4096)

        readLoop: while true {
          let count: Int = readBuffer.withUnsafeMutableBytes { pointer in
            while true {
              let result = Darwin.read(descriptor, pointer.baseAddress, pointer.count)
              if result < 0, errno == EINTR { continue }
              return result
            }
          }
          guard count > 0 else { break readLoop }
          buffer.append(contentsOf: readBuffer[0..<count])

          while let newlineIndex = buffer.firstIndex(of: 0x0A) {
            let line = buffer.subdata(in: buffer.startIndex..<newlineIndex)
            buffer.removeSubrange(buffer.startIndex...newlineIndex)
            if !line.isEmpty { continuation.yield(line) }
          }
        }
        continuation.finish()
      }
      thread.name = "classick.DaemonClient.reader"
      thread.start()
    }
  }

  static func connect(path: String) -> Int32? {
    let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
    guard descriptor >= 0 else { return nil }

    var noSigPipe: Int32 = 1
    setsockopt(
      descriptor, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe,
      socklen_t(MemoryLayout<Int32>.size))

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let pathBytes = Array(path.utf8)
    guard pathBytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
      close(descriptor)
      return nil
    }
    withUnsafeMutableBytes(of: &address.sun_path) { raw in
      let buffer = raw.bindMemory(to: UInt8.self)
      for (index, byte) in pathBytes.enumerated() { buffer[index] = byte }
      buffer[pathBytes.count] = 0
    }

    let length = socklen_t(MemoryLayout<sockaddr_un>.size)
    let result = withUnsafePointer(to: &address) { pointer in
      pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        Darwin.connect(descriptor, $0, length)
      }
    }
    guard result == 0 else {
      close(descriptor)
      return nil
    }
    return descriptor
  }

  static func writeAll(_ data: Data, to descriptor: Int32) -> Bool {
    data.withUnsafeBytes { pointer in
      guard let baseAddress = pointer.baseAddress else { return true }
      var offset = 0
      while offset < pointer.count {
        let count = Darwin.write(
          descriptor, baseAddress.advanced(by: offset), pointer.count - offset)
        if count < 0, errno == EINTR { continue }
        guard count > 0 else { return false }
        offset += count
      }
      return true
    }
  }
}

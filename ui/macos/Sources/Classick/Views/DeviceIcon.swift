import SwiftUI

struct DeviceIcon: View {
    var drive: String?
    var size: CGFloat

    @MainActor private static var cache: [String: NSImage] = [:]

    var body: some View {
        Group {
            if let icon = resolvedIcon() {
                Image(nsImage: icon)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
            } else {
                Image(systemName: "ipod")
                    .font(.system(size: size * 0.85))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: size, height: size)
    }

    private func resolvedIcon() -> NSImage? {
        let key = drive ?? "<disconnected>"
        if let cached = Self.cache[key] { return cached }
        let modelNum = drive
            .flatMap { try? String(contentsOfFile: $0 + "/iPod_Control/Device/SysInfo", encoding: .utf8) }
            .flatMap(DeviceIconLogic.parseModelNum)
        let resource = DeviceIconLogic.ampResourcesDir
            + "/\(DeviceIconLogic.iconBaseName(modelNum: modelNum)).icns"
        let icon = NSImage(contentsOfFile: resource) ?? drive.flatMap { path in
            FileManager.default.fileExists(atPath: path) ? NSWorkspace.shared.icon(forFile: path) : nil
        }
        if let icon { Self.cache[key] = icon }
        return icon
    }
}

enum DeviceIconLogic {
    nonisolated static let ampResourcesDir =
        "/System/Library/PrivateFrameworks/AMPDevices.framework/Versions/A/Resources"

    nonisolated static func parseModelNum(sysInfo: String) -> String? {
        for line in sysInfo.split(whereSeparator: \.isNewline) {
            let parts = line.split(separator: ":", maxSplits: 1)
            guard parts.count == 2,
                  parts[0].trimmingCharacters(in: .whitespaces) == "ModelNumStr" else { continue }
            var value = parts[1].trimmingCharacters(in: .whitespaces).uppercased()
            if value.hasPrefix("X") { value.removeFirst() }
            if value.hasPrefix("M") { value.removeFirst() }
            let model = String(value.prefix(while: { $0.isLetter || $0.isNumber }).prefix(4))
            return model.count == 4 ? model : nil
        }
        return nil
    }

    nonisolated static func iconBaseName(modelNum: String?) -> String {
        modelNum.flatMap { table[$0] } ?? "iPod11-Silver"
    }

    nonisolated static var allIconBaseNames: Set<String> { Set(table.values) }

    private nonisolated static let table: [String: String] = [
        "8513": "iPod1", "8541": "iPod1", "8697": "iPod1", "8709": "iPod1",
        "8737": "iPod1", "8740": "iPod1", "8738": "iPod1", "8741": "iPod1",
        "8976": "iPod2", "8946": "iPod2", "9460": "iPod2", "9244": "iPod2",
        "8948": "iPod2", "9245": "iPod2",
        "9282": "iPod4-White", "9268": "iPod4-White", "9787": "iPod4-BlackRed",
        "9160": "iPod3-Silver", "9436": "iPod3-Blue", "9435": "iPod3-Pink",
        "9434": "iPod3-Green", "9437": "iPod3-Gold",
        "9800": "iPod3-Silver", "9802": "iPod3B-Blue", "9804": "iPod3B-Pink",
        "9806": "iPod3B-Green", "9801": "iPod3-Silver", "9803": "iPod3B-Blue",
        "9805": "iPod3B-Pink", "9807": "iPod3B-Green",
        "A079": "iPod4-White", "9829": "iPod4-White", "9585": "iPod4-White",
        "9830": "iPod4-White", "9586": "iPod4-White", "A127": "iPod4-BlackRed",
        "E436": "iPod4-White", "S492": "iPod4-White",
        "A002": "iPod5-White", "A003": "iPod5-White",
        "A146": "iPod6-Black", "A147": "iPod6-Black", "A452": "iPod5-BlackRed",
        "A444": "iPod6-White", "A448": "iPod6-White",
        "A446": "iPod6-Black", "A450": "iPod6-Black", "A664": "iPod6-BlackRed",
        "A350": "iPod7-White", "A004": "iPod7-White", "A005": "iPod7-White",
        "A352": "iPod7-Black", "A099": "iPod7-Black", "A107": "iPod7-Black",
        "A477": "iPod9-Silver", "A426": "iPod9-Silver", "A428": "iPod9-Blue",
        "A487": "iPod9-Green", "A489": "iPod9-Pink", "A725": "iPod9-Red",
        "A726": "iPod9-Red", "A497": "iPod9-Black",
        "A978": "iPod12-Silver", "A980": "iPod12-Silver", "B261": "iPod12-Black",
        "B249": "iPod12-Blue", "B253": "iPod12-Green", "B257": "iPod12-Red",
        "B480": "iPod15-Silver", "B651": "iPod15-Blue", "B654": "iPod15-Pink",
        "B657": "iPod15-Purple", "B660": "iPod15-Orange", "B663": "iPod15-Green",
        "B666": "iPod15-Yellow", "B598": "iPod15-Silver", "B732": "iPod15-Blue",
        "B735": "iPod15-Pink", "B739": "iPod15-Purple", "B742": "iPod15-Orange",
        "B745": "iPod15-Green", "B748": "iPod15-Yellow", "B751": "iPod15-Red",
        "B754": "iPod15-Black", "B903": "iPod15-Silver", "B905": "iPod15-Blue",
        "B907": "iPod15-Pink", "B909": "iPod15-Purple", "B911": "iPod15-Orange",
        "B913": "iPod15-Green", "B915": "iPod15-Yellow", "B917": "iPod15-Red",
        "B918": "iPod15-Black",
        "C027": "iPod16-Silver", "C031": "iPod16-Black", "C034": "iPod16-Purple",
        "C037": "iPod16-Blue", "C040": "iPod16-Green", "C043": "iPod16-Yellow",
        "C046": "iPod16-Orange", "C049": "iPod16-Red", "C050": "iPod16-Pink",
        "C060": "iPod16-Silver", "C062": "iPod16-Black", "C064": "iPod16-Purple",
        "C066": "iPod16-Blue", "C068": "iPod16-Green", "C070": "iPod16-Yellow",
        "C072": "iPod16-Orange", "C074": "iPod16-Red", "C075": "iPod16-Pink",
        "C525": "iPod17-Silver", "C688": "iPod17-DarkGray", "C689": "iPod17-Blue",
        "C690": "iPod17-Green", "C691": "iPod17-Orange", "C692": "iPod17-Pink",
        "C693": "iPod17-Red", "C526": "iPod17-Silver", "C694": "iPod17-DarkGray",
        "C695": "iPod17-Blue", "C696": "iPod17-Green", "C697": "iPod17-Orange",
        "C698": "iPod17-Pink", "C699": "iPod17-Red",
        "B029": "iPod11-Silver", "B145": "iPod11-Silver", "B147": "iPod11-Black",
        "B150": "iPod11-Black", "B562": "iPod11-Silver", "B565": "iPod11-Black",
        "C293": "iPod11-Silver", "C297": "iPod11B-Black",
    ]
}

// GitHub Actions (macOS runner): print the CGWindowID of the largest on-screen, normal-layer
// window owned by a process, or exit 1 when it has none. Used by macos-launch-screenshot.sh
// to capture the app's own window (`screencapture -l`). With --list, print every window the
// process owns, on screen or not (id, layer, on-screen flag, bounds, title), for the log.
//
//   swiftc -O -o window-id ci/github/macos-window-id.swift && ./window-id [--list] PID
import CoreGraphics
import Foundation

let arguments = Array(CommandLine.arguments.dropFirst())
let list = arguments.first == "--list"
guard let pid = Int(arguments.last ?? ""), arguments.count == (list ? 2 : 1) else {
    FileHandle.standardError.write(Data("usage: window-id [--list] PID\n".utf8))
    exit(2)
}
let options: CGWindowListOption = list ? [.optionAll] : [.optionOnScreenOnly, .excludeDesktopElements]
let owned = (CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []).filter {
    ($0[kCGWindowOwnerPID as String] as? Int) == pid
}
func bounds(_ window: [String: Any]) -> [String: Double] {
    window[kCGWindowBounds as String] as? [String: Double] ?? [:]
}
if list {
    for window in owned {
        let b = bounds(window)
        print(
            "window \(window[kCGWindowNumber as String] ?? "?") layer \(window[kCGWindowLayer as String] ?? "?")",
            "onscreen \(window[kCGWindowIsOnscreen as String] ?? false)",
            "at \(b["X"] ?? 0),\(b["Y"] ?? 0) size \(b["Width"] ?? 0)x\(b["Height"] ?? 0)",
            "title \"\(window[kCGWindowName as String] ?? "")\"")
    }
    exit(0)
}
func area(_ window: [String: Any]) -> Double { (bounds(window)["Width"] ?? 0) * (bounds(window)["Height"] ?? 0) }
let normal = owned.filter { ($0[kCGWindowLayer as String] as? Int) == 0 }
guard let largest = normal.max(by: { area($0) < area($1) }), let id = largest[kCGWindowNumber as String] as? Int
else { exit(1) }
print(id)

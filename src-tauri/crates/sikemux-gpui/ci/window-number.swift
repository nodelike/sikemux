// window-number.swift PID: prints the number of PID's first on-screen window, for `screencapture -l`.
import CoreGraphics

let pid = Int(CommandLine.arguments[1])!
let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
if let window = windows.first(where: { $0[kCGWindowOwnerPID as String] as? Int == pid && $0[kCGWindowLayer as String] as? Int == 0 }),
   let number = window[kCGWindowNumber as String] as? Int {
    print(number)
} else {
    exit(1)
}

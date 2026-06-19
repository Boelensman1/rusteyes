import AppKit
import CoreGraphics
import Foundation

private struct HelperScaffold {
    let appType: NSApplication.Type
    let mainDisplayID: CGDirectDisplayID
    let processID: Int32
}

_ = HelperScaffold(
    appType: NSApplication.self,
    mainDisplayID: CGMainDisplayID(),
    processID: ProcessInfo.processInfo.processIdentifier
)

print("hello world")

// Usage: airdrop-helper /path/a.jpg /path/b.jpg
// Opens the macOS AirDrop picker for the given files, exits when done or cancelled.
import AppKit

final class Delegate: NSObject, NSApplicationDelegate, NSSharingServiceDelegate {
    let urls: [URL]
    init(urls: [URL]) { self.urls = urls }

    func applicationDidFinishLaunching(_ n: Notification) {
        NSApp.activate(ignoringOtherApps: true)
        guard let s = NSSharingService(named: .sendViaAirDrop), s.canPerform(withItems: urls) else {
            fputs("AirDrop is not available\n", stderr); exit(1)
        }
        s.delegate = self
        s.perform(withItems: urls)
    }
    func sharingService(_ s: NSSharingService, didShareItems items: [Any]) { exit(0) }
    func sharingService(_ s: NSSharingService, didFailToShareItems items: [Any], error: Error) {
        fputs("\(error.localizedDescription)\n", stderr); exit(1)
    }
}

let urls = CommandLine.arguments.dropFirst().map { URL(fileURLWithPath: $0) }
if urls.isEmpty { fputs("no files\n", stderr); exit(2) }
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = Delegate(urls: Array(urls))
app.delegate = delegate
app.run()

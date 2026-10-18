import Cocoa

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 180, y: 180, width: 520, height: 250),
                      styleMask: [.titled, .closable], backing: .buffered, defer: false)
window.title = "Minutes Shared Context Fixture"
let label = NSTextField(labelWithString: "Disposable shared-context test")
label.frame = NSRect(x: 24, y: 192, width: 470, height: 28)
let price = NSSlider(value: 25, minValue: 0, maxValue: 100, target: nil, action: nil)
price.frame = NSRect(x: 24, y: 128, width: 450, height: 28)
price.setAccessibilityLabel("Price")
let output = NSTextField(labelWithString: "Revenue: 50")
output.frame = NSRect(x: 24, y: 80, width: 450, height: 28)
window.contentView?.addSubview(label)
window.contentView?.addSubview(price)
window.contentView?.addSubview(output)
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
Timer.scheduledTimer(withTimeInterval: 8, repeats: false) { _ in
    price.doubleValue = 75
    output.stringValue = "Revenue: 150"
    NSAccessibility.post(element: price, notification: .valueChanged)
    NSAccessibility.post(element: output, notification: .valueChanged)
}
Timer.scheduledTimer(withTimeInterval: 18, repeats: false) { _ in
    window.title = "Minutes Shared Context Fixture - Different Document"
    NSAccessibility.post(element: window, notification: .titleChanged)
}
Timer.scheduledTimer(withTimeInterval: 28, repeats: false) { _ in app.terminate(nil) }
app.run()

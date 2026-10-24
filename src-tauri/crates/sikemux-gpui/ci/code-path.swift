// code-path.swift PID: prints the path macOS's code signing names for PID, the one an unsigned core
// compares with the places it trusts as its window.
import Foundation
import Security

let attributes = [kSecGuestAttributePid: Int32(CommandLine.arguments[1])!] as CFDictionary
var code: SecCode?
var staticCode: SecStaticCode?
var url: CFURL?
guard SecCodeCopyGuestWithAttributes(nil, attributes, [], &code) == errSecSuccess, let code,
      SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
      SecCodeCopyPath(staticCode, [], &url) == errSecSuccess, let url
else { exit(1) }
print((url as URL).resolvingSymlinksInPath().path)

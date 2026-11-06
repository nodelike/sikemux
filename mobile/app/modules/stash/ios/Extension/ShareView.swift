import SwiftUI

struct ShareView: View {
  @ObservedObject var model: ShareModel
  let cancel: () -> Void

  var body: some View {
    VStack(spacing: 0) {
      header
      switch model.phase {
      case .loading:
        Spacer()
      case .signedOut:
        Settled(icon: Icons.brand, tone: Theme.secondary, title: "Sign in to Sikemux first", detail: "Open the app and sign in, then share again.")
      case let .done(title, detail):
        Settled(icon: Icons.check, tone: Theme.live, title: title, detail: detail)
      case let .failed(problem):
        Settled(icon: Icons.globe, tone: Theme.secondary, title: "Couldn't stash this", detail: problem, retry: model.retry)
      case .ready, .sending:
        form
      }
    }
    .background(Theme.ground.ignoresSafeArea())
    .preferredColorScheme(.dark)
  }

  private var sending: Double? {
    if case let .sending(share) = model.phase { return share }
    return nil
  }

  private var header: some View {
    HStack {
      Button(action: cancel) {
        Text("Cancel").font(Theme.ui(16)).foregroundColor(Theme.secondary)
      }
      .frame(width: 70, alignment: .leading)
      Spacer()
      Text("Sikemux").font(Theme.semibold(16)).foregroundColor(Theme.ink)
      Spacer()
      Color.clear.frame(width: 70, height: 1)
    }
    .padding(.horizontal, 16)
    .frame(height: 50)
  }

  private var form: some View {
    VStack(spacing: 0) {
      ScrollView {
        VStack(alignment: .leading, spacing: 0) {
          Preview(model: model, progress: sending)
          if !model.tooBig.isEmpty {
            Text("\(model.tooBig.count == 1 ? model.tooBig[0] : "\(model.tooBig.count) files") \(model.tooBig.count == 1 ? "is" : "are") over 25 MB, too big to stash")
              .font(Theme.ui(13.5)).foregroundColor(Theme.danger).padding(.top, 8)
          }
          Group {
            TextField("", text: $model.note, prompt: Text("Add a note").foregroundColor(Theme.tertiary), axis: .vertical)
              .lineLimit(1 ... 5)
              .font(Theme.ui(15))
              .foregroundColor(Theme.ink)
              .tint(Theme.accent)
              .padding(.horizontal, 14)
              .padding(.vertical, 12)
              .background(RoundedRectangle(cornerRadius: 12).fill(Theme.sunken))
              .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.border, lineWidth: 1))
              .padding(.top, 10)
            if !model.chats.isEmpty || model.newChat != nil {
              SectionLabel(text: "To a chat")
              VStack(spacing: 2) {
                ForEach(model.chats) { chat in
                  ChatRow(chat: chat) { model.toChat(chat) }
                }
                if let newChat = model.newChat {
                  NewChatRow(chat: newChat) { model.toNewChat(newChat) }
                }
              }
              .padding(.horizontal, -8)
            }
          }
          .opacity(sending == nil ? 1 : 0.45)
          .disabled(sending != nil || model.sendable.isEmpty)
        }
        .padding(.horizontal, 16)
        .padding(.top, 2)
        .padding(.bottom, 16)
      }
      .scrollDismissesKeyboard(.interactively)
      VStack(spacing: 0) {
        Theme.border.frame(height: 1)
        Button(action: model.toStash) {
          HStack(spacing: 8) {
            if sending != nil {
              ProgressView().tint(Theme.ground).controlSize(.small)
            } else {
              IconView(icon: Icons.stack, size: 17)
            }
            Text(sending == nil ? "Stash" : "Stashing").font(Theme.semibold(16))
          }
          .foregroundColor(Color(hex: "#0f0f13"))
          .frame(maxWidth: .infinity, minHeight: 50)
          .background(RoundedRectangle(cornerRadius: 12).fill(Theme.ink))
        }
        .buttonStyle(Pressed())
        .disabled(sending != nil || model.sendable.isEmpty)
        .opacity(model.sendable.isEmpty ? 0.45 : 1)
        .padding(.horizontal, 16)
        .padding(.top, 10)
        .padding(.bottom, 12)
      }
    }
  }
}

private struct Pressed: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label.opacity(configuration.isPressed ? 0.8 : 1)
  }
}

private struct Preview: View {
  @ObservedObject var model: ShareModel
  let progress: Double?

  var body: some View {
    VStack(spacing: 0) {
      content
      if let progress {
        GeometryReader { frame in
          ZStack(alignment: .leading) {
            Theme.border
            Theme.ink.frame(width: frame.size.width * progress)
          }
        }
        .frame(height: 3)
      }
    }
    .background(Theme.raised)
    .clipShape(RoundedRectangle(cornerRadius: 14))
    .overlay(RoundedRectangle(cornerRadius: 14).stroke(Theme.border, lineWidth: 1))
  }

  @ViewBuilder private var content: some View {
    switch model.items.first {
    case let .link(url)?:
      VStack(alignment: .leading, spacing: 0) {
        if let picture = model.picture {
          Image(uiImage: picture).resizable().scaledToFill().frame(height: 128).frame(maxWidth: .infinity).clipped()
        }
        VStack(alignment: .leading, spacing: 4) {
          Text(model.title ?? url.absoluteString.replacingOccurrences(of: "https://", with: ""))
            .font(Theme.semibold(14.5)).foregroundColor(Theme.ink).lineLimit(2)
          HStack(spacing: 6) {
            IconView(icon: Icons.globe, size: 12).foregroundColor(Theme.tertiary)
            Text(url.host ?? url.absoluteString).font(Theme.ui(12.5)).foregroundColor(Theme.tertiary).lineLimit(1)
          }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, 11)
      }
    case let .text(text)?:
      Text(text).font(Theme.ui(14.5)).foregroundColor(Theme.secondary).lineLimit(6)
        .frame(maxWidth: .infinity, alignment: .leading).padding(14)
    default:
      files
    }
  }

  private var files: some View {
    let all = model.items.compactMap { item -> (String, String, Int64)? in
      if case let .file(_, name, mime, size) = item { return (name, mime, size) }
      return nil
    }
    let pictures = all.filter { $0.1.hasPrefix("image/") }.count
    let title = all.count == 1 ? all[0].0 : pictures == all.count ? "\(all.count) photos" : "\(all.count) files"
    let size = all.reduce(Int64(0)) { $0 + $1.2 }
    return HStack(spacing: 12) {
      if model.thumbnails.isEmpty {
        IconView(icon: Icons.file, size: 20).foregroundColor(Theme.secondary)
          .frame(width: 52, height: 52)
          .background(RoundedRectangle(cornerRadius: 10).fill(Theme.ground))
          .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.border, lineWidth: 1))
      } else {
        HStack(spacing: 6) {
          ForEach(Array(model.thumbnails.enumerated()), id: \.offset) { _, image in
            Image(uiImage: image).resizable().scaledToFill().frame(width: 72, height: 72).clipShape(RoundedRectangle(cornerRadius: 10))
          }
        }
      }
      VStack(alignment: .leading, spacing: 4) {
        Text(title).font(all.count == 1 && pictures == 0 ? Theme.mono(13) : Theme.semibold(14.5)).foregroundColor(Theme.ink).lineLimit(2)
        Text(ByteCountFormatter.string(fromByteCount: size, countStyle: .file)).font(Theme.ui(12.5)).foregroundColor(Theme.tertiary)
      }
      Spacer(minLength: 0)
    }
    .padding(10)
  }
}

private struct SectionLabel: View {
  let text: String

  var body: some View {
    HStack(spacing: 10) {
      Text(text.uppercased()).font(Theme.medium(11)).tracking(1.76).foregroundColor(Theme.tertiary)
      Theme.rule.frame(height: 1)
    }
    .padding(.horizontal, 4)
    .padding(.top, 18)
    .padding(.bottom, 4)
  }
}

private struct ChatRow: View {
  let chat: ShareChat
  let pick: () -> Void

  var body: some View {
    Button(action: pick) {
      HStack(spacing: 10) {
        AgentMark(provider: chat.provider, size: 20)
        VStack(alignment: .leading, spacing: 1) {
          Text(chat.title).font(Theme.medium(14.5)).foregroundColor(Theme.ink).lineLimit(1)
          HStack(spacing: 4) {
            IconView(icon: Icons.folder, size: 11).foregroundColor(Theme.live)
            Text("\(chat.project) · \(chat.hostName)").font(Theme.ui(12)).foregroundColor(Theme.tertiary).lineLimit(1)
          }
        }
        Spacer(minLength: 8)
        if let name = chat.name {
          Text(name).font(Theme.ui(11.5)).foregroundColor(Theme.tertiary).lineLimit(1)
        }
      }
      .padding(.horizontal, 8)
      .padding(.vertical, 6)
      .frame(minHeight: 48)
      .contentShape(Rectangle())
    }
    .buttonStyle(RowPress())
  }
}

private struct NewChatRow: View {
  let chat: ShareNewChat
  let pick: () -> Void

  var body: some View {
    Button(action: pick) {
      HStack(spacing: 10) {
        IconView(icon: Icons.plus, size: 18).foregroundColor(Theme.secondary).frame(width: 20, height: 20)
        VStack(alignment: .leading, spacing: 1) {
          (Text("New chat in ").foregroundColor(Theme.secondary) + Text(chat.project).foregroundColor(Theme.ink))
            .font(Theme.medium(14.5)).lineLimit(1)
          Text(chat.hostName).font(Theme.ui(12)).foregroundColor(Theme.tertiary).lineLimit(1)
        }
        Spacer(minLength: 0)
      }
      .padding(.horizontal, 8)
      .padding(.vertical, 6)
      .frame(minHeight: 48)
      .contentShape(Rectangle())
    }
    .buttonStyle(RowPress())
  }
}

private struct RowPress: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label.background(RoundedRectangle(cornerRadius: 9).fill(configuration.isPressed ? Theme.ink.opacity(0.1) : .clear))
  }
}

private struct Settled: View {
  let icon: IconDef
  let tone: Color
  let title: String
  let detail: String
  var retry: (() -> Void)?

  var body: some View {
    VStack(spacing: 6) {
      Spacer()
      IconView(icon: icon, size: 22)
        .foregroundColor(tone)
        .frame(width: 44, height: 44)
        .background(RoundedRectangle(cornerRadius: 12).fill(tone == Theme.live ? Theme.live.opacity(0.07) : Theme.raised))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(tone == Theme.live ? Theme.live.opacity(0.4) : Theme.border, lineWidth: 1))
        .padding(.bottom, 12)
      Text(title).font(Theme.semibold(19)).foregroundColor(Theme.ink).multilineTextAlignment(.center)
      Text(detail).font(Theme.ui(14)).foregroundColor(Theme.tertiary).multilineTextAlignment(.center)
      if let retry {
        Button(action: retry) {
          Text("Try again").font(Theme.semibold(14)).foregroundColor(Theme.ink)
            .padding(.horizontal, 13).frame(height: 32)
            .background(RoundedRectangle(cornerRadius: 10).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderStrong, lineWidth: 1))
        }
        .padding(.top, 12)
      }
      Spacer()
      Spacer()
    }
    .padding(.horizontal, 32)
    .frame(maxWidth: .infinity)
  }
}

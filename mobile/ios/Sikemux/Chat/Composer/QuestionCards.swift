import SwiftUI

/// The questions at the end of a chat: those answered until the agent reads them, the open ones, and those the agent
/// went on without on an assumption.
struct ChatQuestionsFooter: View {
    let core: String
    let agentId: String
    let session: ChatSession
    /// A watching phone shows the questions without the answers.
    var canAnswer = true
    var focus: ComposerFocus?

    var body: some View {
        let questions = ChatQuestions.of(core, agentId)
        VStack(alignment: .leading, spacing: 0) {
            ForEach(questions.answered, id: \.id) { question in
                AnsweredQuestion(
                    own: question.agentId == agentId, asker: question.name, cli: question.cli,
                    question: question.asked, answer: question.answer ?? "", secret: question.secret
                )
            }
            QuestionDock(questions: questions, canAnswer: canAnswer) { question in
                questions.write(question)
                focus?.request()
            }
            ForEach(questions.assumed, id: \.id) { question in
                AssumedQuestion(
                    question: question, canAnswer: canAnswer,
                    onFine: { questions.answer(question, .fine) },
                    onChange: {
                        questions.write(question)
                        focus?.request()
                    }
                )
            }
        }
    }
}

/// Who is asking, above the question itself, and the ticket of the board card it works; `onFold` adds the chevron
/// that folds the questions away.
private struct AskedBy: View {
    @Environment(\.palette) private var palette
    let cli: String
    var ticket: String?
    var onFold: (() -> Void)?
    let words: Text

    var body: some View {
        HStack(spacing: 7) {
            AgentMark(provider: cli, size: 14)
            words.font(Theme.ui(11.5)).foregroundStyle(palette.inkDim).lineLimit(1)
            Spacer(minLength: 0)
            if let ticket { Text(ticket).font(Theme.mono(11)).foregroundStyle(palette.inkDim) }
            if let onFold {
                Button(action: onFold) {
                    IconView(icon: Icons.chevron, size: 10)
                        .foregroundStyle(palette.inkFaint)
                        .rotationEffect(.degrees(90))
                        .frame(width: 24, height: 20)
                        .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .accessibilityLabel("Fold the questions away")
            }
        }
        .padding(.top, 14)
        .padding(.bottom, 2)
    }
}

private func named(_ name: String, _ palette: Palette) -> Text {
    Text(name).font(Theme.ui(11.5, .semibold)).foregroundColor(palette.ink)
}

/// A question as it reads once answered: its words, and the answer in one line, or only that it was answered when
/// the answer is a secret.
struct AnsweredQuestion: View {
    @Environment(\.palette) private var palette
    /// This chat's own agent asked, so who asked goes without saying.
    let own: Bool
    let asker: String
    let cli: String
    let question: String
    let answer: String
    let secret: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !own { AskedBy(cli: cli, words: named(asker, palette) + Text(" asked you")) }
            Text(question).font(Theme.ui(14.5)).lineSpacing(23 - 14.5 * 1.2).foregroundStyle(palette.ink).textSelection(.enabled)
            if secret {
                Private(text: "Answered, kept private")
            } else {
                (Text("You answered: ") + Text(answer).foregroundColor(palette.ink))
                    .font(Theme.ui(13))
                    .lineSpacing(19 - 13 * 1.2)
                    .foregroundStyle(palette.inkDim)
                    .textSelection(.enabled)
                    .padding(.top, 4)
                    .padding(.bottom, 8)
            }
        }
    }
}

private struct Private: View {
    @Environment(\.palette) private var palette
    let text: String

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            IconView(icon: Icons.lock, size: 12).foregroundStyle(palette.inkFaint).padding(.top, 3)
            Text(text).font(Theme.ui(12.5)).lineSpacing(18 - 12.5 * 1.2).foregroundStyle(palette.inkDim)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.top, 4)
        .padding(.bottom, 8)
    }
}

/// The answer to a secret question, typed or pasted into a field that keeps nothing once it is held. A secure field
/// cannot hold line breaks, so this one draws its text invisibly.
private struct SecretAnswer: View {
    @Environment(\.palette) private var palette
    let question: AgentQuestion
    let onKeep: (String) -> Bool
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 8) {
                TextField("", text: $text, prompt: Text("Type or paste the answer").foregroundColor(palette.tertiary), axis: .vertical)
                    .font(Theme.ui(16))
                    .foregroundStyle(.clear)
                    .tint(palette.accent)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .textContentType(.oneTimeCode)
                    .lineLimit(1...5)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .frame(minHeight: 40)
                    .background(RoundedRectangle(cornerRadius: Radius.row, style: .continuous).fill(palette.sunken))
                    .overlay(RoundedRectangle(cornerRadius: Radius.row, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
                    .accessibilityLabel("Private answer to \(question.name)")
                Chip(title: "Send", primary: true) {
                    if !text.isEmpty, onKeep(text) { text = "" }
                }
            }
            if !text.isEmpty {
                Text(hiddenSize).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim)
            }
            Private(text: question.secretNote ?? "")
        }
        .padding(.top, 8)
        .padding(.bottom, 6)
    }

    /// How much a hidden answer holds, so the person can see their typing and paste landed.
    private var hiddenSize: String {
        let lines = text.hasSuffix("\n") ? String(text.dropLast()).components(separatedBy: "\n").count : text.components(separatedBy: "\n").count
        let characters = "\(text.count) character\(text.count == 1 ? "" : "s") hidden"
        return lines > 1 ? "\(lines) lines, \(characters)" : characters
    }
}

/// A question's name on its tab: the words the agent gave it, else its place among the rest.
private func title(_ question: AgentQuestion, _ index: Int, _ count: Int) -> String {
    question.header ?? (count > 1 ? "Question \(index + 1)" : "Question")
}

/// The questions an agent put to the person and waits on, at the end of its chat: a tab for each when there are
/// several, the words of the one on show, then its answers as rows with a ring, or a tick on a question that takes
/// several. "Something else…" hands over to the composer; a secret is typed into its own masked field instead.
struct QuestionDock: View {
    @Environment(\.palette) private var palette
    let questions: ChatQuestions
    let canAnswer: Bool
    let onWrite: (AgentQuestion) -> Void

    var body: some View {
        if let current = questions.current {
            if questions.folded {
                folded
            } else {
                dock(current)
            }
        }
    }

    private var folded: some View {
        let open = questions.open
        let names = open.enumerated().map { title($1, $0, open.count) }.joined(separator: " · ")
        return Button {
            Haptics.select()
            questions.fold(false)
        } label: {
            HStack(spacing: 8) {
                Text(open.count == 1 ? "QUESTION" : "QUESTIONS").font(Theme.ui(11, .medium)).tracking(0.6).foregroundStyle(palette.inkFaint)
                Text("\(open.count)").font(Theme.mono(11)).foregroundStyle(palette.inkDim)
                Text(names).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).lineLimit(1)
                Spacer(minLength: 0)
                IconView(icon: Icons.chevron, size: 10).foregroundStyle(palette.inkFaint)
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .padding(.top, 14)
        .padding(.bottom, 6)
        .accessibilityLabel("\(open.count) \(open.count == 1 ? "question" : "questions"): \(names). Open them")
    }

    private func dock(_ current: AgentQuestion) -> some View {
        let open = questions.open
        let picked = questions.picks[current.id]
        var written: ChatQuestions.Pick?
        if case .written = picked?.given { written = picked }
        let writing = questions.writing && questions.composing?.id == current.id
        let sends = open.count > 1 || current.multi || written != nil
        let via = current.viaNote.map { " · \($0)" } ?? ""
        return VStack(alignment: .leading, spacing: 0) {
            AskedBy(cli: current.cli, ticket: current.ticket, onFold: { questions.fold(true) },
                    words: named(current.name, palette) + Text(" asks you\(via)"))
            if open.count > 1 { tabs(current) }
            (Text(current.text) + (current.multi ? Text(" Pick any.").foregroundColor(palette.inkFaint) : Text("")))
                .font(Theme.ui(14.5))
                .lineSpacing(23 - 14.5 * 1.2)
                .foregroundStyle(palette.ink)
                .textSelection(.enabled)
            if canAnswer && (!current.choices.isEmpty || !current.secret) {
                VStack(spacing: 2) {
                    ForEach(Array(current.choices.enumerated()), id: \.offset) { index, choice in
                        let on = chosen(picked, index)
                        row(on: on, multi: current.multi, mark: AnyView(Key(multi: current.multi, on: on)),
                            label: choiceLabel(choice),
                            a11y: [choice.label, choice.recommended == true ? "recommended" : nil, choice.detail].compactMap { $0 }.joined(separator: ", ")) {
                            questions.choose(current, index)
                        }
                    }
                    if !current.secret {
                        if let written {
                            row(on: true, multi: current.multi, mark: pencil, label: Text(written.shown), a11y: "Your answer, \(written.shown)") { onWrite(current) }
                        } else {
                            row(on: writing, multi: current.multi, mark: pencil,
                                label: Text("Something else…").foregroundColor(palette.inkDim)
                                    + (writing ? Text("   answering below").font(Theme.ui(13)).foregroundColor(palette.inkDim) : Text("")),
                                a11y: "Something else") { onWrite(current) }
                        }
                    }
                }
                .padding(.top, 6)
                .padding(.bottom, 4)
                .padding(.horizontal, -10)
            }
            if canAnswer && current.secret {
                if picked != nil {
                    Private(text: "Answer kept, sent with the rest")
                } else {
                    SecretAnswer(question: current) { questions.hold(current, WrittenAnswer(answer: $0)) }
                }
            }
            if canAnswer && sends {
                HStack {
                    Spacer()
                    Chip(title: "Send", primary: questions.ready, disabled: !questions.ready) { questions.sendAll() }
                }
                .padding(.bottom, 8)
            }
        }
    }

    private var pencil: AnyView {
        AnyView(IconView(icon: Icons.pencil, size: 12).foregroundStyle(palette.inkFaint).frame(width: 14, height: 20))
    }

    private func chosen(_ picked: ChatQuestions.Pick?, _ index: Int) -> Bool {
        switch picked?.given {
        case .choice(let choice): choice == index
        case .choices(let choices): choices.contains(index)
        default: false
        }
    }

    private func choiceLabel(_ choice: QuestionChoice) -> Text {
        var label = Text(choice.label)
        if choice.recommended == true { label = label + Text("  recommended").font(Theme.ui(12)).foregroundColor(palette.inkFaint) }
        if let detail = choice.detail { label = label + Text("   \(detail)").font(Theme.ui(13)).foregroundColor(palette.inkDim) }
        return label
    }

    private func row(on: Bool, multi: Bool, mark: AnyView, label: Text, a11y: String, _ action: @escaping () -> Void) -> some View {
        Button {
            Haptics.select()
            action()
        } label: {
            HStack(alignment: .top, spacing: 12) {
                mark
                label.font(Theme.ui(14.5)).lineSpacing(20 - 14.5 * 1.2).foregroundStyle(palette.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(10)
            .frame(minHeight: 40)
            .contentShape(Rectangle())
        }
        .buttonStyle(QuestionRowStyle(on: on))
        .accessibilityLabel(a11y)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private func tabs(_ current: AgentQuestion) -> some View {
        let open = questions.open
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(Array(open.enumerated()), id: \.element.id) { index, question in
                    let on = question.id == current.id
                    let answered = questions.picks[question.id] != nil
                    Button {
                        Haptics.select()
                        questions.show(question)
                    } label: {
                        HStack(spacing: 6) {
                            if answered {
                                IconView(icon: Icons.check, size: 11).foregroundStyle(palette.inkFaint)
                            } else {
                                Text("\(index + 1)").font(Theme.mono(11)).foregroundStyle(palette.inkFaint)
                            }
                            Text(title(question, index, open.count))
                                .font(Theme.ui(12.5))
                                .foregroundStyle(on ? palette.ink : palette.secondary)
                                .lineLimit(1)
                                .frame(maxWidth: 180)
                                .fixedSize()
                        }
                        .padding(.horizontal, 10)
                        .frame(height: 30)
                        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(on ? palette.active : .clear))
                        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(on ? .clear : palette.border, lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(title(question, index, open.count))\(answered ? ", answered" : "")")
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            .padding(.horizontal, 18)
        }
        .padding(.horizontal, -18)
        .padding(.top, 8)
        .padding(.bottom, 6)
    }
}

private struct QuestionRowStyle: ButtonStyle {
    @Environment(\.palette) private var palette
    let on: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label.background(
            RoundedRectangle(cornerRadius: 10, style: .continuous).fill(on || configuration.isPressed ? palette.active : .clear)
        )
    }
}

/// The ring of a choice on a question that takes one, or the box of one on a question that takes several.
private struct Key: View {
    @Environment(\.palette) private var palette
    let multi: Bool
    let on: Bool

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: multi ? 4 : 7, style: .continuous)
        ZStack {
            shape.fill(on && multi ? palette.ink : .clear)
            shape.strokeBorder(on ? palette.ink : palette.borderStrong, lineWidth: 1.5)
            if on {
                if multi {
                    IconView(icon: Icons.check, size: 10).foregroundStyle(palette.ground)
                } else {
                    Circle().fill(palette.ink).frame(width: 6, height: 6)
                }
            }
        }
        .frame(width: 14, height: 14)
        .padding(.top, 3)
    }
}

/// A question the agent went on without, on the assumption it stated: Fine lets it stand, Change answers it.
struct AssumedQuestion: View {
    @Environment(\.palette) private var palette
    let question: AgentQuestion
    let canAnswer: Bool
    let onFine: () -> Void
    let onChange: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            IconView(icon: Icons.check, size: 13).foregroundStyle(palette.inkDim).frame(height: 18)
            Text(words)
                .font(Theme.ui(12.5))
                .lineSpacing(18 - 12.5 * 1.2)
                .foregroundStyle(palette.inkDim)
                .tint(palette.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .environment(\.openURL, OpenURLAction { url in
                    Haptics.select()
                    if url.host() == "fine" { onFine() } else { onChange() }
                    return .handled
                })
        }
        .padding(.top, 10)
        .padding(.bottom, 6)
    }

    /// The assumption in one run of words, with Fine and Change as taps inside it.
    private var words: AttributedString {
        var name = AttributedString(question.name)
        name.font = Theme.ui(12.5, .semibold)
        name.foregroundColor = palette.ink
        var words = name + AttributedString(" assumed \(question.assumption ?? "")")
        guard canAnswer else { return words }
        words += AttributedString("  ·  ")
        for (label, host) in [("Fine", "fine"), ("Change", "change")] {
            var act = AttributedString(label)
            act.font = Theme.ui(12.5, .medium)
            act.foregroundColor = palette.ink
            act.link = URL(string: "sikemux-act://\(host)")
            words += act
            if host == "fine" { words += AttributedString("   ") }
        }
        return words
    }
}

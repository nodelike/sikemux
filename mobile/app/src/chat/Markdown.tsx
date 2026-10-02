import { Fragment, useMemo, type ReactNode } from 'react';
import { Linking, ScrollView, StyleSheet, Text, View, type TextStyle } from 'react-native';

import { fonts, type Palette, useStyles } from '@/ui/theme';

type Block =
  | { kind: 'paragraph'; text: string }
  | { kind: 'heading'; text: string }
  | { kind: 'item'; marker: string; text: string }
  | { kind: 'code'; text: string };

const FENCE = /^\s*```/;

function blocks(source: string): Block[] {
  const out: Block[] = [];
  const lines = source.split('\n');
  let paragraph: string[] = [];
  const flush = () => {
    if (paragraph.length) out.push({ kind: 'paragraph', text: paragraph.join(' ') });
    paragraph = [];
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (FENCE.test(line)) {
      flush();
      const code: string[] = [];
      for (index += 1; index < lines.length && !FENCE.test(lines[index]); index += 1) code.push(lines[index]);
      out.push({ kind: 'code', text: code.join('\n') });
      continue;
    }
    const heading = line.match(/^#{1,6}\s+(.*)$/);
    const bullet = line.match(/^\s*[-*+]\s+(.*)$/);
    const numbered = line.match(/^\s*(\d+)[.)]\s+(.*)$/);
    if (heading) {
      flush();
      out.push({ kind: 'heading', text: heading[1] });
    } else if (bullet) {
      flush();
      out.push({ kind: 'item', marker: '•', text: bullet[1] });
    } else if (numbered) {
      flush();
      out.push({ kind: 'item', marker: `${numbered[1]}.`, text: numbered[2] });
    } else if (!line.trim()) {
      flush();
    } else {
      paragraph.push(line.trim());
    }
  }
  flush();
  return out;
}

/**
 * How much of a message can no longer change as it streams in: everything up to its last
 * blank line or closed code fence. Blocks before that point parse the same whatever follows.
 */
function settledLength(source: string): number {
  let settled = 0;
  let fenced = false;
  for (let start = 0, end = source.indexOf('\n'); end !== -1; start = end + 1, end = source.indexOf('\n', start)) {
    const line = source.slice(start, end);
    if (FENCE.test(line)) {
      fenced = !fenced;
      if (!fenced) settled = end + 1;
    } else if (!fenced && !line.trim()) {
      settled = end + 1;
    }
  }
  return settled;
}

const LINK = /^\[([^\]]+)\]\(([^)\s]+)\)$/;
const OPENABLE = /^(https?:|mailto:)/i;

function Link({ url, children, styles }: { url: string; children: string; styles: Styles }) {
  return (
    <Text style={styles.link} onPress={() => Linking.openURL(url).catch(() => {})} accessibilityRole="link">
      {children}
    </Text>
  );
}

/** `code` as the Mac's purple chip, **bold** as semibold, links in the accent; everything else as written. */
function inline(text: string, styles: Styles): ReactNode[] {
  return text.split(/(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^)\s]+\)|https?:\/\/[^\s<>()]+)/g).map((piece, index) => {
    if (piece.startsWith('`') && piece.endsWith('`') && piece.length > 1) {
      return (
        <Text key={index} style={styles.chip}>
          {piece.slice(1, -1)}
        </Text>
      );
    }
    if (piece.startsWith('**') && piece.endsWith('**') && piece.length > 4) {
      return (
        <Text key={index} style={styles.bold}>
          {piece.slice(2, -2)}
        </Text>
      );
    }
    const link = piece.match(LINK);
    if (link) {
      return OPENABLE.test(link[2]) ? (
        <Link key={index} url={link[2]} styles={styles}>
          {link[1]}
        </Link>
      ) : (
        <Fragment key={index}>{link[1]}</Fragment>
      );
    }
    if (/^https?:\/\//.test(piece)) {
      const url = piece.replace(/[.,;:!?'"]+$/, '');
      return (
        <Fragment key={index}>
          <Link url={url} styles={styles}>
            {url}
          </Link>
          {piece.slice(url.length)}
        </Fragment>
      );
    }
    return <Fragment key={index}>{piece}</Fragment>;
  });
}

function Blocks({ blocks, first, style, styles }: { blocks: Block[]; first: number; style: TextStyle; styles: Styles }) {
  return blocks.map((block, index) => {
    const key = first + index;
    switch (block.kind) {
      case 'code':
        return (
          <ScrollView key={key} horizontal style={styles.code} contentContainerStyle={{ padding: 10 }}>
            <Text style={styles.codeText} selectable>
              {block.text}
            </Text>
          </ScrollView>
        );
      case 'heading':
        return (
          <Text key={key} style={[style, styles.bold]} selectable>
            {inline(block.text, styles)}
          </Text>
        );
      case 'item':
        return (
          <View key={key} style={styles.item}>
            <Text style={[style, styles.marker]}>{block.marker}</Text>
            <Text style={[style, { flex: 1 }]} selectable>
              {inline(block.text, styles)}
            </Text>
          </View>
        );
      default:
        return (
          <Text key={key} style={style} selectable>
            {inline(block.text, styles)}
          </Text>
        );
    }
  });
}

export function Markdown({ text, style }: { text: string; style: TextStyle }) {
  const source = text.replace(/\r\n/g, '\n');
  const cut = settledLength(source);
  return <Parsed settled={source.slice(0, cut)} tail={source.slice(cut)} style={style} />;
}

function Parsed({ settled, tail, style }: { settled: string; tail: string; style: TextStyle }) {
  const styles = useStyles(makeStyles);
  const done = useMemo(() => blocks(settled), [settled]);
  return (
    <View style={styles.stack}>
      <Blocks blocks={done} first={0} style={style} styles={styles} />
      <Blocks blocks={blocks(tail)} first={done.length} style={style} styles={styles} />
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    stack: { gap: 10 },
    chip: { fontFamily: fonts.mono, fontSize: 13, color: colors.accent, backgroundColor: colors.accentSoft },
    bold: { fontFamily: fonts.uiSemibold, color: colors.ink },
    link: { color: colors.accent },
    item: { flexDirection: 'row', gap: 8, paddingLeft: 2 },
    marker: { color: colors.tertiary, minWidth: 14 },
    code: { borderRadius: 8, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken },
    codeText: { fontFamily: fonts.mono, fontSize: 12, lineHeight: 18, color: colors.ink },
  });

type Styles = ReturnType<typeof makeStyles>;

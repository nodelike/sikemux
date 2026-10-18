import { Fragment, memo, useMemo, type ReactNode } from 'react';
import { Linking, ScrollView, StyleSheet, Text, View, type StyleProp, type TextStyle } from 'react-native';

import { fencedDiff } from '@mac/chat/diff';
import { namesWholePath } from '@mac/chat/filePath';
import { decodedFenceName } from '@mac/chat/transcript';
import { safeWebUrl } from '@mac/lib/urls';
import { CopyButton } from '@/ui/CopyButton';
import { Icon } from '@/ui/Icon';
import { fonts, italic, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { DiffRow, SidewaysRows } from './Diff';
import { FileRef, fileRuns, hostImagePath, LinkedImage, namesFile } from './FileRef';
import { ImageViewerHost } from './ImageViewer';
import { blocks, inlines, settledLength, type Block, type Inline } from './markdownBlocks';
import type { Fading } from './reveal';

/** A link the phone opens: the web links the Mac would open, and mail. */
const openable = (url: string) => safeWebUrl(url) ?? (/^mailto:/i.test(url) ? url : null);

function Link({ url, children, styles }: { url: string; children: ReactNode; styles: Styles }) {
  return (
    <Text style={styles.link} onPress={() => Linking.openURL(url).catch(() => {})} accessibilityRole="link">
      {children}
    </Text>
  );
}

/** Prose with the files it names drawn as the Mac's file refs. */
function proseRuns(text: string, key: number): ReactNode {
  const runs = fileRuns(text);
  if (runs.length === 1 && typeof runs[0] === 'string') return <Fragment key={key}>{text}</Fragment>;
  return (
    <Fragment key={key}>
      {runs.map((run, index) => (typeof run === 'string' ? run : <FileRef key={index} raw={run.raw} label={run.raw} look="link" />))}
    </Fragment>
  );
}

/**
 * `code` as the Mac's purple chip, **bold** as semibold, links in the accent, files as file refs and
 * pictures on the host in place; everything else as written. A link's own words are left as written.
 */
function renderInline(nodes: Inline[], styles: Styles, paths = true): ReactNode[] {
  return nodes.map((node, index) => {
    if (typeof node === 'string') return paths ? proseRuns(node, index) : <Fragment key={index}>{node}</Fragment>;
    switch (node.kind) {
      case 'code':
        if (namesWholePath(node.text) && namesFile(node.text)) return <FileRef key={index} raw={node.text} label={node.text} look="code" />;
        return (
          <Text key={index} style={styles.chip}>
            {node.text}
          </Text>
        );
      case 'bold':
        return (
          <Text key={index} style={styles.bold}>
            {renderInline(node.children, styles, paths)}
          </Text>
        );
      case 'italic':
        return (
          <Text key={index} style={italic}>
            {renderInline(node.children, styles, paths)}
          </Text>
        );
      case 'strike':
        return (
          <Text key={index} style={styles.strike}>
            {renderInline(node.children, styles, paths)}
          </Text>
        );
      case 'link': {
        const label = renderInline(node.children, styles, false);
        const url = openable(node.url);
        if (url)
          return (
            <Link key={index} url={url} styles={styles}>
              {label}
            </Link>
          );
        const image = hostImagePath(node.url);
        if (image) return <LinkedImage key={index} path={image} label={label} />;
        if (namesWholePath(node.url)) return <FileRef key={index} raw={node.url} label={label} look="link" />;
        return <Fragment key={index}>{label}</Fragment>;
      }
    }
  });
}

const inline = (text: string, styles: Styles) => renderInline(inlines(text), styles);

/** A line whose newest words are still fading in: the settled start as markdown, then each word at its own strength. */
function lineWithFading(text: string, fading: Fading | undefined, style: StyleProp<TextStyle>, styles: Styles): ReactNode {
  const coming =
    fading
      ?.map((piece) => piece.text)
      .join('')
      .trimEnd() ?? '';
  if (!fading?.length || !coming || !text.endsWith(coming)) return inline(text, styles);
  const ink = StyleSheet.flatten(style)?.color;
  let left = coming.length;
  return (
    <>
      {inline(text.slice(0, text.length - coming.length), styles)}
      {fading.map((piece, index) => {
        const words = piece.text.slice(0, left);
        left -= words.length;
        return (
          <Text key={index} style={typeof ink === 'string' && { color: translucent(ink, piece.opacity) }}>
            {words.replace(/[*~]/g, '')}
          </Text>
        );
      })}
    </>
  );
}

const HEADING_SIZES = [20, 18, 16.5];

/* A table cell is sized from its text up front, so a wide table lays out in one pass
   and scrolls sideways in its own box instead of measuring every cell. */
const CELL_CHAR = 7.4;
const CELL_PAD = 20;
const CELL_MIN = 56;
const CELL_MAX = 240;

const COPY_ROOM = 28;

/* While a view around a scroller holds the touch, iOS won't let the scroller pan, so the
   scroller's own content takes it before the message's press handler can. */
const keepTouch = () => true;

function columnWidths(block: Extract<Block, { kind: 'table' }>): number[] {
  return block.head.map((head, column) => {
    let chars = head.length;
    for (const row of block.rows) chars = Math.max(chars, row[column].replace(/[`*_~]/g, '').length);
    const room = column === block.head.length - 1 ? COPY_ROOM : 0;
    return Math.min(CELL_MAX, Math.max(CELL_MIN, Math.ceil(chars * CELL_CHAR) + CELL_PAD)) + room;
  });
}

const escapeCell = (cell: string) => cell.replace(/\|/g, '\\|');

function tableSource(block: Extract<Block, { kind: 'table' }>): string {
  const row = (cells: string[]) => `| ${cells.map(escapeCell).join(' | ')} |`;
  const rule = block.align.map((align) => (align === 'center' ? ':---:' : align === 'right' ? '---:' : align === 'left' ? ':---' : '---'));
  return [row(block.head), `| ${rule.join(' | ')} |`, ...block.rows.map(row)].join('\n');
}

function Table({ block, style, styles }: { block: Extract<Block, { kind: 'table' }>; style: StyleProp<TextStyle>; styles: Styles }) {
  const widths = useMemo(() => columnWidths(block), [block]);
  const source = useMemo(() => tableSource(block), [block]);
  const cell = (text: string, column: number, head: boolean) => (
    <View
      key={column}
      style={[
        styles.cell,
        { width: widths[column] },
        column === widths.length - 1 && styles.lastCell,
        head && column === widths.length - 1 && { paddingRight: 10 + COPY_ROOM },
      ]}>
      <Text
        style={[
          style,
          styles.cellText,
          head && styles.headText,
          block.align[column] !== null && { textAlign: block.align[column] ?? undefined },
        ]}
        selectable>
        {inline(text, styles)}
      </Text>
    </View>
  );
  return (
    <View style={styles.table}>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}>
        <View onStartShouldSetResponder={keepTouch}>
          <View style={[styles.row, styles.headRow]}>{block.head.map((text, column) => cell(text, column, true))}</View>
          {block.rows.map((row, index) => (
            <View key={index} style={[styles.row, index === block.rows.length - 1 && styles.lastRow]}>
              {row.map((text, column) => cell(text, column, false))}
            </View>
          ))}
        </View>
      </ScrollView>
      <CopyButton value={source} label="table" size={12} style={styles.tableCopy} />
    </View>
  );
}

/**
 * A fence as the Mac draws it: a title bar with the file it quotes and its copy button, then the code,
 * or a patch's rows tinted by sign with the changed words marked.
 */
function CodeBlock({ lang, text, styles }: { lang: string; text: string; styles: Styles }) {
  const colors = useColors();
  const patch = useMemo(() => (text ? fencedDiff(text, lang || undefined) : null), [text, lang]);
  const name = lang ? decodedFenceName(lang) : '';
  return (
    <View style={styles.code}>
      <View style={styles.codeTitle}>
        {name ? <Icon name="IconFile" size={11} color={colors.inkFaint} /> : null}
        <Text style={styles.codeName} numberOfLines={1}>
          {name}
        </Text>
        <CopyButton value={text.replace(/\n$/, '')} label="code" size={12} />
      </View>
      {patch ? (
        <SidewaysRows>
          {patch.map((line, index) => (
            <DiffRow key={index} line={line} numbered={false} />
          ))}
        </SidewaysRows>
      ) : (
        <ScrollView horizontal contentContainerStyle={styles.codePad} showsHorizontalScrollIndicator={false}>
          <View onStartShouldSetResponder={keepTouch}>
            <Text style={styles.codeText} selectable>
              {text}
            </Text>
          </View>
        </ScrollView>
      )}
    </View>
  );
}

function sameBlock(before: Block, after: Block): boolean {
  if (before === after) return true;
  if (before.kind !== after.kind) return false;
  switch (before.kind) {
    case 'rule':
      return true;
    case 'table':
      return JSON.stringify(before) === JSON.stringify(after);
    case 'item': {
      const item = after as typeof before;
      return before.text === item.text && before.marker === item.marker && before.depth === item.depth && before.checked === item.checked;
    }
    case 'heading':
      return before.text === (after as typeof before).text && before.level === (after as typeof before).level;
    case 'code':
      return before.text === (after as typeof before).text && before.lang === (after as typeof before).lang;
    default:
      return before.text === (after as typeof before).text;
  }
}

/** One block, drawn again only when its own text changes, so a streaming message redraws just its tail. */
const BlockView = memo(
  function BlockView({ block, style, styles, fading }: { block: Block; style: StyleProp<TextStyle>; styles: Styles; fading?: Fading }) {
    switch (block.kind) {
      case 'code':
        return <CodeBlock lang={block.lang} text={block.text} styles={styles} />;
      case 'heading':
        return (
          <Text
            style={[
              style,
              styles.bold,
              block.level <= HEADING_SIZES.length && {
                fontSize: HEADING_SIZES[block.level - 1],
                lineHeight: HEADING_SIZES[block.level - 1] * 1.35,
              },
            ]}
            accessibilityRole="header"
            selectable>
            {lineWithFading(block.text, fading, style, styles)}
          </Text>
        );
      case 'item':
        return (
          <View style={[styles.item, { paddingLeft: 2 + block.depth * 18 }]}>
            {block.checked === null ? (
              <Text style={[style, styles.marker]}>{block.marker}</Text>
            ) : (
              <View style={[styles.box, block.checked && styles.boxOn]} accessibilityLabel={block.checked ? 'done' : 'not done'}>
                {block.checked ? <Icon name="IconCheck" size={11} color={styles.boxTick.color} /> : null}
              </View>
            )}
            <Text style={[style, { flex: 1 }, block.checked === true && styles.done]} selectable>
              {lineWithFading(block.text, fading, style, styles)}
            </Text>
          </View>
        );
      case 'quote':
        return (
          <View style={styles.quote}>
            <Markdown text={block.text} style={[style, styles.quoteText]} />
          </View>
        );
      case 'rule':
        return <View style={styles.rule} />;
      case 'table':
        return <Table block={block} style={style} styles={styles} />;
      default:
        return (
          <Text style={style} selectable>
            {lineWithFading(block.text, fading, style, styles)}
          </Text>
        );
    }
  },
  (before, after) =>
    before.style === after.style &&
    before.styles === after.styles &&
    before.fading === after.fading &&
    sameBlock(before.block, after.block),
);

/** Markdown text; `fading` is its end, still coming in, drawn fainter word by word. */
export function Markdown({ text, style, fading }: { text: string; style: StyleProp<TextStyle>; fading?: Fading }) {
  const source = text.replace(/\r\n/g, '\n');
  const cut = settledLength(source);
  return <Parsed settled={source.slice(0, cut)} tail={source.slice(cut)} style={style} fading={fading} />;
}

function Parsed({ settled, tail, style, fading }: { settled: string; tail: string; style: StyleProp<TextStyle>; fading?: Fading }) {
  const styles = useStyles(makeStyles);
  const done = useMemo(() => blocks(settled), [settled]);
  const all = [...done, ...blocks(tail)];
  return (
    <ImageViewerHost>
      <View style={styles.stack}>
        {all.map((block, index) => (
          <BlockView key={index} block={block} style={style} styles={styles} fading={index === all.length - 1 ? fading : undefined} />
        ))}
      </View>
    </ImageViewerHost>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    stack: { gap: 10 },
    chip: { fontFamily: fonts.mono, fontSize: 12.5, color: colors.accent, backgroundColor: colors.accentSoft },
    bold: { fontFamily: fonts.uiSemibold, color: colors.ink },
    link: { color: colors.accent },
    item: { flexDirection: 'row', gap: 8, paddingLeft: 2 },
    marker: { color: colors.tertiary, minWidth: 14 },
    code: { borderRadius: 8, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken, overflow: 'hidden' },
    codeTitle: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      height: 26,
      paddingLeft: 10,
      paddingRight: 1,
      borderBottomWidth: 1,
      borderBottomColor: colors.border,
    },
    codeName: { flex: 1, fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
    codePad: { paddingHorizontal: 12, paddingVertical: 9 },
    codeText: { fontFamily: fonts.mono, fontSize: 11.5, lineHeight: 17, color: colors.ink },
    strike: { textDecorationLine: 'line-through', color: colors.tertiary },
    box: {
      width: 15,
      height: 15,
      marginTop: 4,
      marginRight: 1,
      borderRadius: 4,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      alignItems: 'center',
      justifyContent: 'center',
    },
    boxOn: { borderColor: colors.borderSelected, backgroundColor: colors.selected },
    boxTick: { color: colors.accent },
    done: { color: colors.tertiary },
    quote: { paddingLeft: 12, borderLeftWidth: 2, borderLeftColor: colors.borderStrong },
    quoteText: { color: colors.secondary },
    rule: { height: StyleSheet.hairlineWidth, marginVertical: 4, backgroundColor: colors.borderStrong },
    table: { borderRadius: 8, borderWidth: 1, borderColor: colors.border, overflow: 'hidden' },
    row: { flexDirection: 'row', borderBottomWidth: 1, borderBottomColor: colors.border },
    headRow: { backgroundColor: colors.active, borderBottomColor: colors.borderStrong },
    lastRow: { borderBottomWidth: 0 },
    cell: { paddingHorizontal: 10, paddingVertical: 6, borderRightWidth: 1, borderRightColor: colors.border },
    lastCell: { borderRightWidth: 0 },
    cellText: { fontSize: 13.5, lineHeight: 19 },
    headText: { fontFamily: fonts.uiSemibold, fontSize: 12, letterSpacing: 0.6, textTransform: 'uppercase', color: colors.secondary },
    tableCopy: { position: 'absolute', top: 3, right: 3, backgroundColor: colors.raised },
  });

type Styles = ReturnType<typeof makeStyles>;

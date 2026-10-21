import { Fragment, memo, useMemo, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View, type StyleProp, type TextStyle } from 'react-native';

import { fencedDiff } from '@mac/chat/diff';
import { namesWholePath } from '@mac/chat/filePath';
import { decodedFenceName } from '@mac/chat/transcript';
import { safeWebUrl } from '@mac/lib/urls';
import { tableSource } from '@mac/markdown/tableSource';
import type { MdElement, MdNode } from '@mac/markdown/types';
import { visibleText } from '@mac/markdown/visible';
import { CopyButton } from '@/ui/CopyButton';
import { Icon } from '@/ui/Icon';
import { openLink } from '@/ui/prompt';
import { fonts, italic, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { DiffRow, SidewaysRows } from './Diff';
import { FileRef, fileRuns, hostImagePath, LinkedImage, namesFile } from './FileRef';
import { ImageViewerHost } from './ImageViewer';
import type { Fading } from './reveal';

type Table = Extract<MdElement, { t: 'table' }>;

/** Text from a host that sends no tree, as the paragraphs its blank lines make. */
export function plainBlocks(text: string): MdElement[] {
  return text
    .replace(/\r\n/g, '\n')
    .split(/\n{2,}/)
    .filter((paragraph) => paragraph.trim())
    .map((paragraph) => ({ t: 'p', c: [paragraph] }));
}

/** A link the phone opens: the web links the Mac would open, and mail. */
const openable = (url: string) => safeWebUrl(url) ?? (/^mailto:/i.test(url) ? url : null);

function Link({ url, children, styles }: { url: string; children: ReactNode; styles: Styles }) {
  return (
    <Text style={styles.link} onPress={() => openLink(url)} accessibilityRole="link">
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

type Ink = { styles: Styles; paths: boolean };

function inline(node: MdNode, key: number, ink: Ink): ReactNode {
  const { styles, paths } = ink;
  if (typeof node === 'string') return paths ? proseRuns(node, key) : <Fragment key={key}>{node}</Fragment>;
  switch (node.t) {
    case 'code':
      if (paths && namesWholePath(node.v) && namesFile(node.v)) return <FileRef key={key} raw={node.v} label={node.v} look="code" />;
      return (
        <Text key={key} style={styles.chip}>
          {node.v}
        </Text>
      );
    case 'strong':
      return (
        <Text key={key} style={styles.bold}>
          {inlines(node.c, ink)}
        </Text>
      );
    case 'em':
      return (
        <Text key={key} style={italic}>
          {inlines(node.c, ink)}
        </Text>
      );
    case 'del':
      return (
        <Text key={key} style={styles.strike}>
          {inlines(node.c, ink)}
        </Text>
      );
    case 'br':
      return <Fragment key={key}>{'\n'}</Fragment>;
    case 'fnref':
      return (
        <Text key={key} style={styles.note}>
          {node.n}
        </Text>
      );
    case 'img': {
      const image = hostImagePath(node.src);
      if (image) return <LinkedImage key={key} path={image} label={node.alt} />;
      const url = openable(node.src);
      return url ? (
        <Link key={key} url={url} styles={styles}>
          {node.alt || node.src}
        </Link>
      ) : (
        <Fragment key={key}>{node.alt}</Fragment>
      );
    }
    case 'a': {
      const label = inlines(node.c, { styles, paths: false });
      const url = openable(node.href);
      if (url)
        return (
          <Link key={key} url={url} styles={styles}>
            {label}
          </Link>
        );
      const image = hostImagePath(node.href);
      if (image) return <LinkedImage key={key} path={image} label={label} />;
      if (namesWholePath(node.href)) return <FileRef key={key} raw={node.href} label={label} look="link" />;
      return <Fragment key={key}>{label}</Fragment>;
    }
    default:
      return 'c' in node ? <Fragment key={key}>{inlines(node.c, ink)}</Fragment> : null;
  }
}

/**
 * `code` as the Mac's purple chip, **bold** as semibold, links in the accent, files as file refs and
 * pictures on the host in place; everything else as written. A link's own words are left as written.
 */
function inlines(nodes: readonly MdNode[], ink: Ink): ReactNode[] {
  return nodes.map((node, index) => inline(node, index, ink));
}

/** A line whose newest words are still fading in: the settled start as written, then each word at its own strength. */
function withFading(nodes: readonly MdNode[], fading: Fading | undefined, style: StyleProp<TextStyle>, styles: Styles): ReactNode {
  const ink = { styles, paths: true };
  const last = nodes.at(-1);
  const coming =
    fading
      ?.map((piece) => piece.text)
      .join('')
      .trimEnd() ?? '';
  if (!fading?.length || !coming || typeof last !== 'string' || !last.endsWith(coming)) return inlines(nodes, ink);
  const color = StyleSheet.flatten(style)?.color;
  let left = coming.length;
  return (
    <>
      {inlines(nodes.slice(0, -1), ink)}
      {proseRuns(last.slice(0, last.length - coming.length), nodes.length)}
      {fading.map((piece, index) => {
        const words = piece.text.slice(0, left);
        left -= words.length;
        return (
          <Text key={`fading-${index}`} style={typeof color === 'string' && { color: translucent(color, piece.opacity) }}>
            {words}
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
/** A heading cell is set in spaced capitals, wider than the rows' text. */
const HEAD_CHAR = 9;
const CELL_PAD = 20;
const CELL_MIN = 56;
const CELL_MAX = 240;

const COPY_ROOM = 28;

/* While a view around a scroller holds the touch, iOS won't let the scroller pan, so the
   scroller's own content takes it before the message's press handler can. */
const keepTouch = () => true;

function columnWidths(table: Table): number[] {
  return table.align.map((_, column) => {
    let text = visibleText(table.head?.[column] ?? []).length * HEAD_CHAR;
    for (const row of table.rows) text = Math.max(text, visibleText(row[column] ?? []).length * CELL_CHAR);
    const room = column === table.align.length - 1 ? COPY_ROOM : 0;
    return Math.min(CELL_MAX, Math.max(CELL_MIN, Math.ceil(text) + CELL_PAD)) + room;
  });
}

function TableView({ table, style, styles }: { table: Table; style: StyleProp<TextStyle>; styles: Styles }) {
  const widths = useMemo(() => columnWidths(table), [table]);
  const source = useMemo(() => tableSource(table), [table]);
  const cell = (nodes: readonly MdNode[], column: number, head: boolean) => (
    <View
      key={column}
      style={[
        styles.cell,
        column === widths.length - 1 ? { minWidth: widths[column], flexGrow: 1 } : { width: widths[column] },
        column === widths.length - 1 && styles.lastCell,
        head && column === widths.length - 1 && { paddingRight: 10 + COPY_ROOM },
      ]}>
      <Text
        style={[
          style,
          styles.cellText,
          head && styles.headText,
          table.align[column] !== null && { textAlign: table.align[column] ?? undefined },
        ]}
        selectable>
        {inlines(nodes, { styles, paths: true })}
      </Text>
    </View>
  );
  return (
    <View style={styles.table}>
      <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.tableFill}>
        <View onStartShouldSetResponder={keepTouch} style={styles.tableFill}>
          {table.head ? (
            <View style={[styles.row, styles.headRow]}>{table.head.map((nodes, column) => cell(nodes, column, true))}</View>
          ) : null}
          {table.rows.map((row, index) => (
            <View key={index} style={[styles.row, index === table.rows.length - 1 && styles.lastRow]}>
              {row.map((nodes, column) => cell(nodes, column, false))}
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

const isBlock = (node: MdNode) =>
  typeof node !== 'string' && ['p', 'h', 'quote', 'ul', 'ol', 'pre', 'hr', 'table', 'fndef', 'details'].includes(node.t);

/** An item's or a footnote's contents: runs of words as one line, blocks under them. */
function Contents({
  nodes,
  style,
  styles,
  fading,
}: {
  nodes: readonly MdNode[];
  style: StyleProp<TextStyle>;
  styles: Styles;
  fading?: Fading;
}) {
  const groups: (MdNode[] | MdElement)[] = [];
  for (const node of nodes) {
    const last = groups.at(-1);
    if (isBlock(node)) groups.push(node as MdElement);
    else if (Array.isArray(last)) last.push(node);
    else groups.push([node]);
  }
  return (
    <View style={styles.contents}>
      {groups.map((group, index) => {
        const tail = index === groups.length - 1 ? fading : undefined;
        return Array.isArray(group) ? (
          <Text key={index} style={style} selectable>
            {withFading(group, tail, style, styles)}
          </Text>
        ) : (
          <BlockView key={index} block={group} style={style} styles={styles} fading={tail} />
        );
      })}
    </View>
  );
}

function ListView({
  items,
  first,
  style,
  styles,
  fading,
}: {
  items: readonly MdNode[];
  /** The first item's number, for a numbered list. */
  first?: number;
  style: StyleProp<TextStyle>;
  styles: Styles;
  fading?: Fading;
}) {
  return (
    <View style={styles.list}>
      {items.map((item, index) => {
        if (typeof item === 'string' || item.t !== 'li') return null;
        const tail = index === items.length - 1 ? fading : undefined;
        return (
          <View key={index} style={styles.item}>
            {item.checked === undefined ? (
              <Text style={[style, styles.marker]}>{first === undefined ? '•' : `${first + index}.`}</Text>
            ) : (
              <View style={[styles.box, item.checked && styles.boxOn]} accessibilityLabel={item.checked ? 'done' : 'not done'}>
                {item.checked ? <Icon name="IconCheck" size={11} color={styles.boxTick.color} /> : null}
              </View>
            )}
            <Contents nodes={item.c} style={[style, item.checked === true && styles.done]} styles={styles} fading={tail} />
          </View>
        );
      })}
    </View>
  );
}

function Details({ block, style, styles }: { block: Extract<MdElement, { t: 'details' }>; style: StyleProp<TextStyle>; styles: Styles }) {
  const colors = useColors();
  const [open, setOpen] = useState(block.open === true);
  return (
    <View style={styles.details}>
      <Pressable onPress={() => setOpen(!open)} style={styles.summary} accessibilityRole="button" accessibilityState={{ expanded: open }}>
        <View style={{ transform: [{ rotate: open ? '90deg' : '0deg' }] }}>
          <Icon name="IconChevron" size={10} color={colors.inkFaint} />
        </View>
        <Text style={[style, styles.bold]}>{block.summary || 'Details'}</Text>
      </Pressable>
      {open ? <Blocks blocks={block.c as MdElement[]} style={style} styles={styles} /> : null}
    </View>
  );
}

/** One block, drawn again only when the core changed it, so a streaming message redraws just its tail. */
const BlockView = memo(function BlockView({
  block,
  style,
  styles,
  fading,
}: {
  block: MdElement;
  style: StyleProp<TextStyle>;
  styles: Styles;
  fading?: Fading;
}) {
  switch (block.t) {
    case 'pre':
      return <CodeBlock lang={block.lang ?? ''} text={block.v} styles={styles} />;
    case 'h':
      return (
        <Text
          style={[
            style,
            styles.bold,
            block.l <= HEADING_SIZES.length && { fontSize: HEADING_SIZES[block.l - 1], lineHeight: HEADING_SIZES[block.l - 1] * 1.35 },
          ]}
          accessibilityRole="header"
          selectable>
          {withFading(block.c, fading, style, styles)}
        </Text>
      );
    case 'ul':
      return <ListView items={block.c} style={style} styles={styles} fading={fading} />;
    case 'ol':
      return <ListView items={block.c} first={block.start ?? 1} style={style} styles={styles} fading={fading} />;
    case 'quote':
      return (
        <View style={styles.quote}>
          <Blocks blocks={block.c as MdElement[]} style={[style, styles.quoteText]} styles={styles} fading={fading} />
        </View>
      );
    case 'hr':
      return <View style={styles.rule} />;
    case 'table':
      return <TableView table={block} style={style} styles={styles} />;
    case 'fndef':
      return (
        <View style={styles.item}>
          <Text style={[style, styles.marker, styles.note]}>{block.n}</Text>
          <Contents nodes={block.c} style={[style, styles.footnote]} styles={styles} />
        </View>
      );
    case 'details':
      return <Details block={block} style={style} styles={styles} />;
    case 'p':
      return (
        <Text style={style} selectable>
          {withFading(block.c, fading, style, styles)}
        </Text>
      );
    default:
      return (
        <Text style={style} selectable>
          {inline(block, 0, { styles, paths: true })}
        </Text>
      );
  }
});

function Blocks({
  blocks,
  style,
  styles,
  fading,
}: {
  blocks: readonly MdElement[];
  style: StyleProp<TextStyle>;
  styles: Styles;
  fading?: Fading;
}) {
  return (
    <View style={styles.stack}>
      {blocks.map((block, index) => (
        <BlockView key={index} block={block} style={style} styles={styles} fading={index === blocks.length - 1 ? fading : undefined} />
      ))}
    </View>
  );
}

/** An answer's blocks; `fading` is its end, still coming in, drawn fainter word by word. */
export function Markdown({ blocks, style, fading }: { blocks: readonly MdElement[]; style: StyleProp<TextStyle>; fading?: Fading }) {
  const styles = useStyles(makeStyles);
  return (
    <ImageViewerHost>
      <Blocks blocks={blocks} style={style} styles={styles} fading={fading} />
    </ImageViewerHost>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    stack: { gap: 10 },
    contents: { flex: 1, gap: 8 },
    list: { gap: 10 },
    note: { fontSize: 10, lineHeight: 14, color: colors.accent },
    footnote: { fontSize: 12.5, color: colors.secondary },
    details: { gap: 8 },
    summary: { flexDirection: 'row', alignItems: 'center', gap: 6 },
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
    tableFill: { flexGrow: 1 },
    cell: { paddingHorizontal: 10, paddingVertical: 6, borderRightWidth: 1, borderRightColor: colors.border },
    lastCell: { borderRightWidth: 0 },
    cellText: { fontSize: 13.5, lineHeight: 19 },
    headText: { fontFamily: fonts.uiSemibold, fontSize: 12, letterSpacing: 0.6, textTransform: 'uppercase', color: colors.secondary },
    tableCopy: { position: 'absolute', top: 3, right: 3, backgroundColor: colors.raised },
  });

type Styles = ReturnType<typeof makeStyles>;

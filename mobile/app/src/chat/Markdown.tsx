import { createContext, Fragment, memo, useContext, useMemo, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View, type StyleProp, type TextStyle } from 'react-native';

import { fencedDiff } from '@shared/chat/diff';
import { decodedFenceName } from '@shared/chat/transcript';
import { safeWebUrl } from '@shared/lib/urls';
import { tableSource } from '@shared/markdown/tableSource';
import type { MdElement, MdNode } from '@shared/markdown/types';
import { visibleText } from '@shared/markdown/visible';
import { useT, type Translate } from '@/i18n';
import { CopyButton } from '@/ui/CopyButton';
import { Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { openLink } from '@/ui/prompt';
import { fonts, italic, mixed, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { keepTouch } from '@/ui/Sideways';
import { DiffRow, SidewaysRows } from './Diff';
import type { FoundCode, FoundNode } from './found';
import { FileRef, hostImagePath, LinkedImage, namesFile } from './FileRef';
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

const SERVICES: [string[], IconName][] = [
  [['github.com'], 'IconGithub'],
  [['bitbucket.org'], 'IconBitbucket'],
  [['atlassian.net'], 'IconJira'],
  [['gitlab.com'], 'IconGitlab'],
  [['codeberg.org'], 'IconForgejo'],
  [['gitea.com'], 'IconGitea'],
  [['linear.app'], 'IconLinear'],
  [['vercel.com', 'vercel.app'], 'IconVercel'],
  [['slack.com'], 'IconSlack'],
  [['aws.amazon.com', 'amazonaws.com'], 'IconAws'],
  [['grok.com'], 'IconGrok'],
  [['apple.com'], 'IconApple'],
  [['play.google.com'], 'IconGoogleplay'],
];

/** The mark of the service a web link leads to, as the Mac puts it before the link. */
function serviceMark(url: string): IconName | undefined {
  const host = /^https?:\/\/([^/?#:]+)/i.exec(url)?.[1]?.toLowerCase();
  if (!host) return undefined;
  return SERVICES.find(([domains]) => domains.some((domain) => host === domain || host.endsWith(`.${domain}`)))?.[1];
}

function Link({ url, children, styles }: { url: string; children: ReactNode; styles: Styles }) {
  const mark = serviceMark(url);
  return (
    <Text style={styles.link} onPress={() => openLink(url)} accessibilityRole="link">
      {mark ? (
        <Text style={styles.mark}>
          <Icon name={mark} size={12} color={styles.link.color} />{' '}
        </Text>
      ) : null}
      {children}
    </Text>
  );
}

type Ink = { styles: Styles; paths: boolean };

const foundOf = (node: MdElement) => ((node as { t: string }).t === 'found' ? (node as unknown as FoundNode) : null);

/** Inline code with the places the find bar found in it marked. */
function FoundIn({ code, styles }: { code: FoundCode; styles: Styles }) {
  const runs: ReactNode[] = [];
  let cursor = 0;
  code.found.forEach(([from, to, current], index) => {
    if (from > cursor) runs.push(code.v.slice(cursor, from));
    runs.push(
      <Text key={index} style={current ? styles.foundCurrent : styles.found}>
        {code.v.slice(from, to)}
      </Text>,
    );
    cursor = to;
  });
  runs.push(code.v.slice(cursor));
  return <Text style={styles.chip}>{runs}</Text>;
}

function inline(node: MdNode, key: number, ink: Ink): ReactNode {
  const { styles, paths } = ink;
  if (typeof node === 'string') return <Fragment key={key}>{node}</Fragment>;
  const found = foundOf(node);
  if (found) {
    return (
      <Text key={key} style={found.current ? styles.foundCurrent : styles.found}>
        {found.c[0]}
      </Text>
    );
  }
  switch (node.t) {
    case 'path': {
      const label = node.code ? visibleText(node.c) : inlines(node.c, { styles, paths: false });
      if (paths && namesFile(node.href)) return <FileRef key={key} raw={node.href} label={label} look={node.code ? 'code' : 'link'} />;
      return node.code ? (
        <Text key={key} style={styles.chip}>
          {label}
        </Text>
      ) : (
        <Fragment key={key}>{label}</Fragment>
      );
    }
    case 'code':
      if ('found' in node) return <FoundIn key={key} code={node as FoundCode} styles={styles} />;
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
    case 'faint':
      return (
        <Text key={key} style={styles.faint}>
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
      if (node.file) return <FileRef key={key} raw={node.href} label={label} look="link" />;
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
      {last.slice(0, last.length - coming.length)}
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

/** Headings as the Mac's editorial reply sets them, against the text's size: ### reads as a spaced-capital label. */
const HEADING_SCALES = [1.52, 1.24, 0.72];
/** The room above a heading that follows other blocks, against the text's size. */
const HEADING_ROOM = [0.61, 1.5, 0.4];

/** How deep a list sits inside other lists; only the outermost one's bullets take the accent. */
const ListDepth = createContext(0);

/* A table cell is sized from its text up front, so a wide table lays out in one pass
   and scrolls sideways in its own box instead of measuring every cell. */
const CELL_CHAR = 7.4;
/** A heading cell is set in spaced capitals, wider than the rows' text. */
const HEAD_CHAR = 9;
const CELL_PAD = 20;
const CELL_MIN = 56;
const CELL_MAX = 240;

const COPY_ROOM = 28;

function columnWidths(table: Table): number[] {
  return table.align.map((_, column) => {
    let text = visibleText(table.head?.[column] ?? []).length * HEAD_CHAR;
    for (const row of table.rows) text = Math.max(text, visibleText(row[column] ?? []).length * CELL_CHAR);
    const room = column === table.align.length - 1 ? COPY_ROOM : 0;
    return Math.min(CELL_MAX, Math.max(CELL_MIN, Math.ceil(text) + CELL_PAD)) + room;
  });
}

function TableView({ table, style, styles }: { table: Table; style: StyleProp<TextStyle>; styles: Styles }) {
  const t = useT();
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
      <CopyButton value={source} label={t('Copy table')} size={12} style={styles.tableCopy} />
    </View>
  );
}

const SHELLS = ['console', 'terminal', 'shell-session', 'shellsession', 'sh-session', 'fish', 'nu', 'ksh', 'csh', 'tcsh'];
const COMMANDS = ['sh', 'bash', 'zsh', 'shell'];

/** What a block without a file name is called, and whether its lines are numbered: commands to type are a Terminal. */
function codeTitle(lang: string, text: string, t: Translate): { title: string; numbered: boolean } {
  const lower = lang.toLowerCase();
  if (lang ? SHELLS.includes(lower) || COMMANDS.includes(lower) : text.startsWith('$ ')) return { title: t('Terminal'), numbered: false };
  return { title: lang || t('Code'), numbered: true };
}

/**
 * A fence as the Mac draws it: one card headed by the file it shows, its folder quiet beside it, and a Copy pill,
 * then the code with its lines numbered, or a patch's rows tinted by sign with the changed words marked.
 */
function CodeBlock({ lang, name, text, styles }: { lang: string; name?: string; text: string; styles: Styles }) {
  const colors = useColors();
  const t = useT();
  const patch = useMemo(() => (text ? fencedDiff(text, lang || undefined) : null), [text, lang]);
  const code = text.replace(/\n$/, '');
  const named = name ? { title: name, numbered: true } : codeTitle(lang ? decodedFenceName(lang) : '', code, t);
  const cut = name ? name.lastIndexOf('/') : -1;
  const [folder, file] = cut > 0 ? [named.title.slice(0, cut), named.title.slice(cut + 1)] : [null, named.title];
  const lines = named.numbered && code.includes('\n') ? code.split('\n').length : 0;
  return (
    <View style={styles.code}>
      <View style={styles.codeHead}>
        <Icon name={named.numbered ? 'IconFile' : 'IconTerminal'} size={13} color={colors.inkFaint} />
        <Text style={styles.codeFile} numberOfLines={1}>
          {file}
        </Text>
        {folder ? (
          <Text style={styles.codeFolder} numberOfLines={1} ellipsizeMode="head">
            {folder}
          </Text>
        ) : null}
        <View style={styles.codeFill} />
        <CopyButton value={code} label={t('Copy code')} worded style={styles.pill} />
      </View>
      {patch ? (
        <SidewaysRows>
          {patch.map((line, index) => (
            <DiffRow key={index} line={line} numbered={false} />
          ))}
        </SidewaysRows>
      ) : (
        <ScrollView
          horizontal
          contentContainerStyle={[styles.codePad, lines > 0 && styles.codePadNumbered]}
          showsHorizontalScrollIndicator={false}>
          <View onStartShouldSetResponder={keepTouch} style={styles.codeLines}>
            {lines > 0 ? (
              <Text style={[styles.codeText, styles.gutter]}>{Array.from({ length: lines }, (_, index) => index + 1).join('\n')}</Text>
            ) : null}
            <Text style={styles.codeText} selectable>
              {code}
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
  const depth = useContext(ListDepth);
  const t = useT();
  return (
    <ListDepth.Provider value={depth + 1}>
      <View style={styles.list}>
        {items.map((item, index) => {
          if (typeof item === 'string' || item.t !== 'li') return null;
          const tail = index === items.length - 1 ? fading : undefined;
          return (
            <View key={index} style={styles.item}>
              {item.checked === undefined ? (
                first === undefined ? (
                  <Text style={[style, styles.bullet, depth > 0 && styles.nestedBullet]}>•</Text>
                ) : (
                  <Text style={[style, styles.number]}>{String(first + index).padStart(2, '0')}</Text>
                )
              ) : (
                <View style={[styles.box, item.checked && styles.boxOn]} accessibilityLabel={item.checked ? t('done') : t('not done')}>
                  {item.checked ? <Icon name="IconCheck" size={11} color={styles.boxTick.color} /> : null}
                </View>
              )}
              <Contents nodes={item.c} style={[style, item.checked === true && styles.done]} styles={styles} fading={tail} />
            </View>
          );
        })}
      </View>
    </ListDepth.Provider>
  );
}

function Details({ block, style, styles }: { block: Extract<MdElement, { t: 'details' }>; style: StyleProp<TextStyle>; styles: Styles }) {
  const colors = useColors();
  const [open, setOpen] = useState(block.open === true);
  const t = useT();
  return (
    <View style={styles.details}>
      <Pressable onPress={() => setOpen(!open)} style={styles.summary} accessibilityRole="button" accessibilityState={{ expanded: open }}>
        <View style={{ transform: [{ rotate: open ? '90deg' : '0deg' }] }}>
          <Icon name="IconChevron" size={10} color={colors.inkFaint} />
        </View>
        <Text style={[style, styles.bold]}>{block.summary || t('Details')}</Text>
      </Pressable>
      {open ? <Blocks blocks={block.c as MdElement[]} style={style} styles={styles} /> : null}
    </View>
  );
}

/** A label's words in capitals, as the Mac sets a ### heading. */
function capitals(nodes: readonly MdNode[]): string {
  return visibleText(nodes as MdElement[]).toUpperCase();
}

/** One block, drawn again only when the core changed it, so a streaming message redraws just its tail. */
const BlockView = memo(function BlockView({
  block,
  style,
  styles,
  fading,
  first = true,
  lead = false,
}: {
  block: MdElement;
  style: StyleProp<TextStyle>;
  styles: Styles;
  fading?: Fading;
  /** Nothing stands above it, so a heading needs no room over it. */
  first?: boolean;
  /** The paragraph straight under the text's title, set a touch larger. */
  lead?: boolean;
}) {
  switch (block.t) {
    case 'pre':
      return <CodeBlock lang={block.lang ?? ''} name={block.name} text={block.v} styles={styles} />;
    case 'h': {
      const size = StyleSheet.flatten(style)?.fontSize ?? 14.5;
      const scale = HEADING_SCALES[block.l - 1];
      const label = block.l === 3;
      return (
        <Text
          style={[
            style,
            styles.bold,
            scale !== undefined && { fontSize: size * scale, lineHeight: size * scale * (label ? 1.6 : 1.3) },
            block.l === 1 && styles.title,
            block.l === 2 && styles.section,
            label && styles.label,
            !first && { marginTop: size * (HEADING_ROOM[block.l - 1] ?? 0.4) },
          ]}
          accessibilityRole="header"
          selectable>
          {label ? capitals(block.c) : withFading(block.c, fading, style, styles)}
        </Text>
      );
    }
    case 'ul':
      return <ListView items={block.c} style={style} styles={styles} fading={fading} />;
    case 'ol':
      return <ListView items={block.c} first={block.start ?? 1} style={style} styles={styles} fading={fading} />;
    case 'quote':
      return (
        <View style={styles.quote}>
          <Blocks blocks={block.c as MdElement[]} style={[style, italic]} styles={styles} fading={fading} />
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
        <Text style={[style, lead && { fontSize: (StyleSheet.flatten(style)?.fontSize ?? 14.5) * 1.07 }]} selectable>
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
        <BlockView
          key={index}
          block={block}
          style={style}
          styles={styles}
          fading={index === blocks.length - 1 ? fading : undefined}
          first={index === 0}
          lead={block.t === 'p' && index > 0 && blocks[index - 1].t === 'h' && (blocks[index - 1] as { l: number }).l === 1}
        />
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
    stack: { gap: 12 },
    contents: { flex: 1, gap: 8 },
    list: { gap: 6 },
    note: { fontSize: 10, lineHeight: 14, color: colors.accent },
    footnote: { fontSize: 12.5, color: colors.secondary },
    details: { gap: 8 },
    summary: { flexDirection: 'row', alignItems: 'center', gap: 6 },
    chip: { fontFamily: fonts.mono, fontSize: 12.5, color: colors.accent, backgroundColor: colors.accentSoft },
    found: { backgroundColor: translucent(colors.accent, 0.34), color: colors.ink },
    foundCurrent: { backgroundColor: colors.accent, color: colors.sunken },
    bold: { fontFamily: fonts.uiSemibold, color: colors.ink },
    link: {
      color: mixed(colors.accent, colors.ink, 0.5),
      textDecorationLine: 'underline',
      textDecorationColor: translucent(mixed(colors.accent, colors.ink, 0.5), 0.4),
    },
    item: { flexDirection: 'row', gap: 8, paddingLeft: 2 },
    marker: { color: colors.tertiary, minWidth: 14 },
    mark: { textDecorationLine: 'none' },
    bullet: { minWidth: 14, paddingLeft: 4, color: colors.accent },
    nestedBullet: { color: colors.inkDim },
    number: { minWidth: 20, fontFamily: fonts.mono, fontSize: 11, fontWeight: '500', color: colors.accent },
    title: { letterSpacing: -0.33 },
    section: { letterSpacing: -0.18 },
    label: { letterSpacing: 1.67, color: colors.inkDim },
    code: { borderRadius: 12, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.raised, overflow: 'hidden' },
    codeHead: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 38, paddingLeft: 14, paddingRight: 8 },
    codeFile: { flexShrink: 0, maxWidth: '60%', fontFamily: fonts.uiMedium, fontSize: 12, color: colors.ink },
    codeFolder: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 11.5, color: colors.inkFaint },
    codeFill: { flex: 1 },
    pill: { borderWidth: 1, borderColor: colors.border, backgroundColor: colors.overlay },
    codePad: { paddingLeft: 16, paddingRight: 16, paddingBottom: 14 },
    codePadNumbered: { paddingLeft: 0 },
    codeLines: { flexDirection: 'row' },
    gutter: { width: 24, marginRight: 16, textAlign: 'right', color: mixed(colors.inkFaint, colors.raised, 0.5) },
    codeText: { fontFamily: fonts.mono, fontSize: 12, lineHeight: 20, color: colors.ink },
    strike: { textDecorationLine: 'line-through', color: colors.tertiary },
    faint: { color: colors.tertiary },
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
    quote: { paddingLeft: 16, borderLeftWidth: 2, borderLeftColor: colors.accent },
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

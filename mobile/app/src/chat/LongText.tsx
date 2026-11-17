import { useContext, useMemo } from 'react';
import { Pressable, StyleSheet, Text } from 'react-native';

import { cutLongBlocks, showRestLabel } from '@shared/chat/longText';
import type { MdElement } from '@shared/markdown/types';
import { revealBlocks, visibleText } from '@shared/markdown/visible';
import type { PartTree } from '@shared/wire.generated';
import { useT } from '@/i18n';
import { fonts, type Palette, useStyles } from '@/ui/theme';
import { FoldsContext, useFold } from './folds';
import { FindingContext, markBlocks } from './found';
import { Markdown, plainBlocks } from './Markdown';
import { useStreamedText } from './reveal';

/**
 * A message too long to draw at once shows its start and the rest on a tap, unless the person
 * watched it stream in.
 */
export function LongText({ id, text, md, live, style }: { id: string; text: string; md?: PartTree; live: boolean; style: object }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  const folds = useContext(FoldsContext);
  const finding = useContext(FindingContext);
  const [whole, setWhole] = useFold(`whole/${id}`, false);
  const tree = useMemo(() => (md?.blocks as MdElement[] | undefined) ?? plainBlocks(text), [md, text]);
  const found = finding?.byPart.get(id);
  const current = finding?.current?.part === id ? finding.current.range : null;
  const blocks = useMemo(() => markBlocks(tree, found, current), [tree, found, current]);
  const plain = useMemo(() => visibleText(blocks), [blocks]);
  const streamed = useStreamedText(id, plain, live);
  if (live) folds.streamed.add(id);
  const cut = whole || found || folds.streamed.has(id) ? null : cutLongBlocks(blocks);
  const shown = streamed.text.length < plain.length ? revealBlocks(blocks, streamed.text.length) : blocks;
  return (
    <>
      {cut ? <Markdown blocks={cut.blocks} style={style} /> : <Markdown blocks={shown} fading={streamed.fading} style={style} />}
      {cut ? (
        <Pressable onPress={() => setWhole(true)} hitSlop={6} style={styles.showRest} accessibilityRole="button">
          <Text style={styles.showRestText}>{showRestLabel(cut.hidden, t)}</Text>
        </Pressable>
      ) : null}
    </>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    showRest: { alignSelf: 'flex-start', marginTop: 6, marginBottom: 4 },
    showRestText: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.tertiary },
  });
};

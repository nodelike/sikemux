import { createContext, memo, type ReactNode, useContext, useEffect, useMemo, useState } from 'react';
import { Animated, Easing } from 'react-native';
import { SvgXml } from 'react-native-svg';

import { buddyStateOfGroup, type BuddyState } from '@mac/buddies/catalogue';
import { drawnFace, type DrawnFace } from '@mac/buddies/faces';
import { buddyMarkup, FULL_BOX } from '@mac/buddies/render';
import type { AgentFace as Face, AgentGroup, FaceStyle } from '@/core/protocol';
import { AgentIcon } from '@/ui/Icon';
import { useAnimating } from '@/ui/motion';

/** The faces the host's agents wear, by agent id, and the style they are drawn in. */
export interface Faces {
  readonly faces: Readonly<Record<string, Face | null>>;
  readonly style: FaceStyle;
}

export const FacesContext = createContext<Faces | undefined>(undefined);

export const useFaces = () => useContext(FacesContext);

type Motion = { axis: 'translateX' | 'translateY'; ms: number; at: number[]; by: number[]; bounce?: boolean };

/* The host app's bob, hop and shake, moving the whole buddy by these many units of its drawing. */
const MOTION: Partial<Record<BuddyState, Motion>> = {
  working: { axis: 'translateY', ms: 700, at: [0, 1], by: [0, -0.8], bounce: true },
  waiting: { axis: 'translateY', ms: 1200, at: [0, 0.25, 0.55, 1], by: [0, -1.8, 0, 0] },
  error: { axis: 'translateX', ms: 500, at: [0, 0.25, 0.5, 0.75, 1], by: [0, 0.5, -0.5, 0.3, 0] },
};

let nextUid = 0;

function Moving({ motion, scale, children }: { motion: Motion; scale: number; children: ReactNode }) {
  const [phase] = useState(() => new Animated.Value(0));
  useEffect(() => {
    const step = (toValue: number) =>
      Animated.timing(phase, {
        toValue,
        duration: motion.ms,
        easing: motion.bounce ? Easing.inOut(Easing.ease) : Easing.linear,
        useNativeDriver: true,
      });
    phase.setValue(0);
    const loop = Animated.loop(motion.bounce ? Animated.sequence([step(1), step(0)]) : step(1));
    loop.start();
    return () => loop.stop();
  }, [phase, motion]);
  const offset = phase.interpolate({ inputRange: motion.at, outputRange: motion.by.map((units) => units * scale) });
  const transform = motion.axis === 'translateX' ? [{ translateX: offset }] : [{ translateY: offset }];
  return <Animated.View style={{ transform }}>{children}</Animated.View>;
}

/** An agent's character as the host's app draws it, moving as it does there while motion is allowed. */
export const Buddy = memo(function Buddy({
  look,
  moves,
  state,
  size,
}: {
  look: DrawnFace;
  moves: boolean;
  state: BuddyState;
  size: number;
}) {
  const [uid] = useState(() => `bd${++nextUid}`);
  const { pack, shape, color } = look;
  const box = look.box ?? FULL_BOX;
  const xml = useMemo(() => buddyMarkup(pack, shape, color, state, uid, box), [pack, shape, color, state, uid, box]);
  const animating = useAnimating();
  const motion = moves && animating ? MOTION[state] : undefined;
  const art = <SvgXml xml={xml} width={size} height={size} />;
  if (!motion) return art;
  const side = Number(box.split(' ')[2]) || 24;
  return (
    <Moving motion={motion} scale={size / side}>
      {art}
    </Moving>
  );
});

/**
 * The face the host gave the agent, otherwise its agent's logo. One the host has not listed, such as a chat about to
 * start, wears the face the host's style gives it while faces are on.
 */
export function AgentFace({
  agentId,
  provider,
  group,
  size,
  logoSize = size,
}: {
  agentId?: string;
  provider: string;
  group?: AgentGroup;
  size: number;
  logoSize?: number;
}) {
  const faces = useFaces();
  const face = agentId ? faces?.faces[agentId] : undefined;
  const wears = face === undefined ? !!faces?.style.on : face !== null;
  const look = useMemo(
    () => (faces && agentId && wears ? drawnFace({ id: agentId }, face, faces.style) : null),
    [faces, agentId, wears, face],
  );
  return look && faces ? (
    <Buddy look={look} moves={faces.style.motion} state={buddyStateOfGroup(group)} size={size} />
  ) : (
    <AgentIcon provider={provider} size={logoSize} />
  );
}

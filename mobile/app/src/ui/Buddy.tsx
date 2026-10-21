import { createContext, memo, type ReactNode, useContext, useEffect, useMemo, useState } from 'react';
import { Animated, Easing } from 'react-native';
import { SvgXml } from 'react-native-svg';

import { buddyStateOfGroup, type BuddyState } from '@mac/buddies/catalogue';
import { publishedBuddy, wearsBuddy, type PublishedBuddies } from '@mac/buddies/published';
import { buddyMarkup } from '@mac/buddies/render';
import type { AgentGroup } from '@/core/protocol';
import { AgentIcon } from '@/ui/Icon';
import { useAnimating } from '@/ui/motion';

/** The buddies the host's app draws. */
export const BuddiesContext = createContext<PublishedBuddies | undefined>(undefined);

export const useBuddies = () => useContext(BuddiesContext);

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
  buddies,
  agentId,
  state,
  size,
}: {
  buddies: PublishedBuddies;
  agentId: string;
  state: BuddyState;
  size: number;
}) {
  const [uid] = useState(() => `bd${++nextUid}`);
  const { pack, shape, color, box } = useMemo(() => publishedBuddy(buddies, agentId), [buddies, agentId]);
  const xml = useMemo(() => buddyMarkup(pack, shape, color, state, uid, box), [pack, shape, color, state, uid, box]);
  const animating = useAnimating();
  const motion = buddies.motion && animating ? MOTION[state] : undefined;
  const art = <SvgXml xml={xml} width={size} height={size} />;
  if (!motion) return art;
  const side = Number(box.split(' ')[2]) || 24;
  return (
    <Moving motion={motion} scale={size / side}>
      {art}
    </Moving>
  );
});

/** The agent's buddy while the host's app draws buddies or the person named it, otherwise its agent's logo, as the app's own faces do. */
export function AgentFace({
  agentId,
  provider,
  named = false,
  group,
  size,
  logoSize = size,
}: {
  agentId?: string;
  provider: string;
  named?: boolean;
  group?: AgentGroup;
  size: number;
  logoSize?: number;
}) {
  const buddies = useBuddies();
  return buddies && agentId && wearsBuddy(buddies.on, named) ? (
    <Buddy buddies={buddies} agentId={agentId} state={buddyStateOfGroup(group)} size={size} />
  ) : (
    <AgentIcon provider={provider} size={logoSize} />
  );
}

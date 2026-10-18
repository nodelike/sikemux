import { Callout as BaseCallout } from "fumadocs-ui/components/callout";
import { Card as BaseCard } from "fumadocs-ui/components/card";
import defaultMdxComponents from "fumadocs-ui/mdx";
import { Bot, Download, Globe, Keyboard } from "lucide-react";
import type { ComponentProps } from "react";
import { AgentMark, agentOf } from "./agent-mark";

const icons = {
  bot: <Bot />,
  download: <Download />,
  globe: <Globe />,
  keyboard: <Keyboard />,
};

function Card({ icon, ...props }: Omit<ComponentProps<typeof BaseCard>, "icon"> & { icon?: keyof typeof icons }) {
  return <BaseCard icon={icon && icons[icon]} {...props} className="doc-card" />;
}

function Callout(props: ComponentProps<typeof BaseCallout>) {
  return <BaseCallout {...props} className="callout" />;
}

const BaseLink = defaultMdxComponents.a;

function Link({ children, ...props }: ComponentProps<"a">) {
  const agent = agentOf(props.href);
  if (!agent) return <BaseLink {...props}>{children}</BaseLink>;
  return (
    <BaseLink {...props} className="agent-link">
      <AgentMark agent={agent} size={14} />
      {children}
    </BaseLink>
  );
}

export const mdxComponents = { ...defaultMdxComponents, a: Link, Callout, Card };

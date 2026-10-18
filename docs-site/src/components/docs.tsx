import { navigate } from "astro:transitions/client";
import type { Folder, Node, Root } from "fumadocs-core/page-tree";
import type { AstroProviderProps } from "fumadocs-core/framework/astro";
import { TOCItem, type TOCItemType } from "fumadocs-core/toc";
import { TOCScrollArea, useTOCItems } from "fumadocs-ui/components/toc";
import { DocsLayout } from "fumadocs-ui/layouts/notebook";
import {
  DocsBody,
  DocsDescription,
  DocsPage,
  DocsTitle,
  EditOnGitHub,
  MarkdownCopyButton,
} from "fumadocs-ui/layouts/notebook/page";
import { TOCPopover, TOCProvider } from "fumadocs-ui/layouts/notebook/page/slots/toc";
import { RootProvider } from "fumadocs-ui/provider/astro";
import { useMemo, type ReactNode } from "react";
import { AgentMark, agentOf } from "./agent-mark";
import { Hero } from "./hero";
import { DiscordIcon, Wordmark } from "./logo";
import Search from "./search";

export function Docs(props: {
  tree: Root;
  pathname: string;
  params: AstroProviderProps["params"];
  title: string;
  description: string;
  toc: TOCItemType[];
  markdownUrl: string;
  editUrl: string;
  kind: "landing" | "page" | "missing";
  children?: ReactNode;
}) {
  const tree = useMemo(() => withIntroduction({ ...props.tree, children: props.tree.children.map(withAgentMarks) }), [props.tree]);
  const agent = agentOf(props.pathname);
  return (
    <RootProvider
      pathname={props.pathname}
      params={props.params}
      navigate={navigate}
      theme={{ enabled: false }}
      search={{ SearchDialog: Search }}
    >
      <DocsLayout
        tree={tree}
        themeSwitch={{ enabled: false }}
        sidebar={{ collapsible: false, components: { Folder: NavGroup } }}
        githubUrl="https://github.com/nodelike/sikemux"
        nav={{ mode: "top", title: <Wordmark /> }}
        links={[
          { text: "sikemux.com", url: "https://sikemux.com", external: true },
          { text: "Changelog", url: "https://sikemux.com/changelog", external: true },
          {
            type: "icon",
            text: "Discord",
            label: "Discord",
            url: "https://discord.gg/UKfmHpF9kX",
            icon: <DiscordIcon />,
            external: true,
          },
        ]}
      >
        {props.kind === "missing" ? (
          <DocsPage
            className="missing"
            breadcrumb={{ enabled: false }}
            footer={{ enabled: false }}
            tableOfContent={{ enabled: false }}
            tableOfContentPopover={{ enabled: false }}
          >
            <p className="hero-label">404</p>
            <DocsTitle>{props.title}</DocsTitle>
            <DocsDescription>{props.description}</DocsDescription>
            <div className="hero-actions">
              <a className="hero-btn hero-btn-primary" href="/">
                Docs home
              </a>
              <a className="hero-btn hero-btn-quiet" href="/getting-started/install/">
                Install
              </a>
            </div>
          </DocsPage>
        ) : props.kind === "landing" ? (
          <DocsPage
            className="landing"
            breadcrumb={{ enabled: false }}
            footer={{ enabled: false }}
            tableOfContent={{ enabled: false }}
            tableOfContentPopover={{ enabled: false }}
          >
            <Hero title={props.title} description={props.description} />
            <DocsBody>{props.children}</DocsBody>
          </DocsPage>
        ) : (
          <DocsPage
            toc={props.toc}
            tableOfContent={{ single: true }}
            slots={{ toc: { provider: TOCProvider, main: Toc, popover: TOCPopover } }}
          >
            <DocsTitle className={agent ? "agent-title" : undefined}>
              {agent && <AgentMark agent={agent} size={28} />}
              {props.title}
            </DocsTitle>
            <DocsDescription>{props.description}</DocsDescription>
            <div className="page-actions">
              <MarkdownCopyButton markdownUrl={props.markdownUrl} />
              <EditOnGitHub href={props.editUrl} />
            </div>
            <DocsBody>{props.children}</DocsBody>
          </DocsPage>
        )}
      </DocsLayout>
    </RootProvider>
  );
}

function withIntroduction(tree: Root): Root {
  const [first, ...rest] = tree.children;
  if (first?.type !== "folder") return tree;
  const introduction: Node = { type: "page", name: "Introduction", url: "/" };
  return { ...tree, children: [{ ...first, children: [introduction, ...first.children] }, ...rest] };
}

function withAgentMarks(node: Node): Node {
  if (node.type === "folder") return { ...node, children: node.children.map(withAgentMarks) };
  if (node.type !== "page") return node;
  const agent = agentOf(node.url);
  return agent ? { ...node, icon: <AgentMark agent={agent} /> } : node;
}

function NavGroup({ item, children }: { item: Folder; children: ReactNode }) {
  return (
    <section className="nav-group">
      <p className="nav-group-title">{item.name}</p>
      <div className="tree">{children}</div>
    </section>
  );
}

function Toc() {
  const items = useTOCItems();
  if (items.length === 0) return <div id="nd-toc-placeholder" className="hidden xl:layout:[--fd-toc-width:268px]" />;
  return (
    <div
      id="nd-toc"
      className="sticky top-(--fd-docs-row-3) [grid-area:toc] h-[calc(var(--fd-docs-height)-var(--fd-docs-row-3))] flex flex-col w-(--fd-toc-width) pt-12 pe-4 pb-2 xl:layout:[--fd-toc-width:268px] max-xl:hidden"
    >
      <p className="toc-title">On this page</p>
      <TOCScrollArea>
        <div className="tree">
          {items.map((item) => (
            <TOCItem key={item.url} href={item.url} data-depth={item.depth}>
              {item.title}
            </TOCItem>
          ))}
        </div>
      </TOCScrollArea>
    </div>
  );
}

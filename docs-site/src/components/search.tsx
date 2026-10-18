import {
  SearchDialog,
  SearchDialogContent,
  SearchDialogFooter,
  SearchDialogInput,
  SearchDialogList,
  SearchDialogListItem,
  SearchDialogOverlay,
  useSearch,
  type SearchItemType,
  type SharedProps,
} from "fumadocs-ui/components/dialog/search";
import { useHighlightQuery, useStaticSearch, type SearchResultItem } from "fumadocs-core/search/client";
import { FileText, Hash, Search as SearchIcon } from "lucide-react";
import { useMemo } from "react";

type Result = { id: string; url: string; type: "page" | "heading" | "text"; content: string; section?: string; last?: boolean };

const SUGGESTED: Result[] = [
  { id: "s-install", url: "/getting-started/install", type: "page", content: "Install", section: "Getting started" },
  { id: "s-first", url: "/getting-started/first-launch", type: "page", content: "First launch", section: "Getting started" },
  { id: "s-agents", url: "/agents/supported", type: "page", content: "Supported agents", section: "Agents" },
  { id: "s-keys", url: "/reference/keyboard-shortcuts", type: "page", content: "Keyboard shortcuts", section: "Reference" },
  { id: "s-cli", url: "/reference/cli", type: "page", content: "CLI", section: "Reference" },
  { id: "s-mcp", url: "/reference/mcp-tools", type: "page", content: "MCP tools", section: "Reference" },
];

function lastRow(markdown: string) {
  return markdown
    .split("\n")
    .filter((line) => line.trim() && !/^[\s|:-]+$/.test(line))
    .at(-1)!;
}

function plain(markdown: string) {
  return markdown
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/`|\*\*?|__/g, "")
    .replace(/^\s*\|\s*|\s*\|\s*$/g, "")
    .replace(/\s*\|\s*/g, " · ")
    .replace(/\s+/g, " ")
    .trim();
}

function flatten(items: SearchResultItem[]): Result[] {
  const results: Result[] = [];
  for (const item of items) {
    const records = item.type === "table" ? item.rows : [item];
    for (const record of records) {
      const section = record.breadcrumbs?.filter((crumb) => crumb !== "Docs").join(" / ");
      results.push({ id: record.id, url: record.url, type: record.type, content: plain(item.type === "table" ? lastRow(record.content) : record.content), section });
    }
  }
  results.forEach((result, i) => {
    result.last = result.type !== "page" && results[i + 1]?.type !== "heading" && results[i + 1]?.type !== "text";
  });
  return results;
}

function Row({ item, onClick }: { item: SearchItemType; onClick: () => void }) {
  const { query } = useSearch();
  const highlight = useHighlightQuery(query);
  const result = item as unknown as Result;
  const child = result.type !== "page";
  return (
    <SearchDialogListItem
      item={item}
      onClick={onClick}
      className="search-row"
      data-kind={result.type}
      data-last={result.last || undefined}
    >
      {child ? (
        <span className="search-tick" aria-hidden="true" />
      ) : (
        <FileText className="search-icon" aria-hidden="true" />
      )}
      {result.type === "heading" && <Hash className="search-icon" aria-hidden="true" />}
      <span ref={highlight} className="search-text">
        {result.content}
      </span>
      {!child && result.section && <span className="search-section">{result.section}</span>}
    </SearchDialogListItem>
  );
}

export default function Search(props: SharedProps) {
  const search = useStaticSearch({});
  const results = useMemo(() => (search.data ? flatten(search.data.items) : undefined), [search.data]);
  return (
    <SearchDialog {...search} {...props}>
      <SearchDialogOverlay className="search-overlay" />
      <SearchDialogContent className="search-dialog">
        <div className="search-head">
          <SearchIcon className="search-head-icon" aria-hidden="true" />
          <SearchDialogInput placeholder="Search the docs" />
          <kbd>esc</kbd>
        </div>
        <SearchDialogList
          className="search-list"
          items={(search.search ? (results ?? null) : SUGGESTED) as SearchItemType[] | null}
          Item={(row) => <Row key={row.item.id} {...row} />}
          Empty={() => <p className="search-empty">No results for “{search.search}”</p>}
        />
        <SearchDialogFooter className="search-foot">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> to move
          </span>
          <span>
            <kbd>↵</kbd> to open
          </span>
        </SearchDialogFooter>
      </SearchDialogContent>
    </SearchDialog>
  );
}

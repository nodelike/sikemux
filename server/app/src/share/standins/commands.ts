/*
 * Stands in for the Mac app's commands in a browser. A shared chat's files are on its sharer's
 * computer, so they open nowhere; a web page opens in a tab.
 */
export function openUrlOnDesk(_agentId: string, url: string) {
  window.open(url, "_blank", "noopener,noreferrer");
}

export function openFileOnDesk() {}

export function requestOpenFile() {}

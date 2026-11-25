import type { OrgRole } from "@sikemux/protocol";

import { settingsFrom } from "../env.ts";
import { ApiFailure, badRequest } from "../http.ts";
import type { Logger } from "../log.ts";

export const DEFAULT_RESEND_FROM = "Sikemux <invites@sikemux.com>";
const RESEND_API_URL = "https://api.resend.com/emails";
const SEND_TIMEOUT_MS = 10_000;

export interface ResendSettings {
  apiKey: string;
  from: string;
}

export interface InviteEmail {
  to: string;
  orgName: string;
  invitedBy: string | null;
  role: OrgRole;
  expiresAt: Date;
  link: string;
}

export interface InviteMailer {
  sendInvite(invite: InviteEmail, log: Logger): Promise<void>;
}

const ROLE_LABELS: Record<OrgRole, string> = {
  owner: "Owner",
  admin: "Admin",
  user: "Member",
};

const LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const TLD = /^(?:[a-z]{2,}|xn--[a-z0-9-]{2,})$/;

export function readResend(
  env: NodeJS.ProcessEnv,
  problems: string[],
): ResendSettings | null {
  const { optional, text } = settingsFrom(env, problems);
  const apiKey = optional("RESEND_API_KEY") ?? null;
  if (apiKey === null) return null;
  const from = text("RESEND_FROM", DEFAULT_RESEND_FROM);
  if (/[\r\n]/.test(from))
    problems.push("RESEND_FROM cannot contain line breaks");
  return { apiKey, from };
}

export function normalizeInviteEmail(raw: string): string {
  const email = raw.trim().toLowerCase();
  if (
    email.length < 3 ||
    email.length > 320 ||
    /[\s<>,"'\\]/.test(email) ||
    hasControlChars(email)
  )
    throw badRequest("Enter a valid email address.");

  const at = email.indexOf("@");
  if (at <= 0 || at !== email.lastIndexOf("@"))
    throw badRequest("Enter a valid email address.");

  const local = email.slice(0, at);
  const domain = email.slice(at + 1);
  if (
    local.length > 64 ||
    local.startsWith(".") ||
    local.endsWith(".") ||
    local.includes("..")
  )
    throw badRequest("Enter a valid email address.");

  const labels = domain.split(".");
  if (
    domain.length > 253 ||
    labels.length < 2 ||
    labels.some((part) => !LABEL.test(part)) ||
    !TLD.test(labels[labels.length - 1] ?? "")
  )
    throw badRequest("Enter a valid email address.");

  return email;
}

function hasControlChars(value: string): boolean {
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}

const BRAND_ICON_URL = "https://sikemux.com/brand/icon-dark-rounded-1024.png";
const FIGTREE_FONT_URL =
  "https://sikemux.com/fonts/Figtree-latin-wght-normal.woff2";
const FONT_STACK =
  "Figtree,-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";
const MONO_STACK = "ui-monospace,'SF Mono',Menlo,Consolas,monospace";

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function formatExpiry(date: Date): string {
  return new Intl.DateTimeFormat("en-GB", {
    day: "numeric",
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  }).format(date);
}

export function buildInviteEmail(invite: InviteEmail): {
  subject: string;
  html: string;
  text: string;
} {
  const safeOrg = invite.orgName.replace(/[\r\n]+/g, " ").trim();
  const safeInviter = invite.invitedBy?.replace(/[\r\n]+/g, " ").trim() || null;
  const roleLabel = ROLE_LABELS[invite.role];
  const expiry = formatExpiry(invite.expiresAt);
  const subject = safeInviter
    ? `${safeInviter} invited you to ${safeOrg} on Sikemux`
    : `You're invited to ${safeOrg} on Sikemux`;

  const introText = safeInviter
    ? `${safeInviter} invited you to join ${safeOrg}. Accept to take one of its seats and get Pro on your own hosts while you hold it.`
    : `You've been invited to join ${safeOrg}. Accept to take one of its seats and get Pro on your own hosts while you hold it.`;

  const introHtml = safeInviter
    ? `<strong style="font-weight:600;color:#0a0a0f;">${escapeHtml(safeInviter)}</strong> invited you to join <strong style="font-weight:600;color:#0a0a0f;">${escapeHtml(safeOrg)}</strong>. Accept to take one of its seats and get Pro on your own hosts while you hold it.`
    : `You've been invited to join <strong style="font-weight:600;color:#0a0a0f;">${escapeHtml(safeOrg)}</strong>. Accept to take one of its seats and get Pro on your own hosts while you hold it.`;

  const escapedLink = escapeHtml(invite.link);
  const escapedRole = escapeHtml(roleLabel);
  const escapedExpiry = escapeHtml(expiry);
  const escapedOrg = escapeHtml(safeOrg);
  const escapedSubject = escapeHtml(subject);

  const headStyles = [
    `@font-face{font-family:Figtree;font-weight:400 700;font-style:normal;font-display:swap;src:url('${FIGTREE_FONT_URL}') format('woff2');}`,
    ":root{color-scheme:light dark;supported-color-schemes:light dark;}",
    "@media (prefers-color-scheme: dark){",
    ".em-bg{background-color:#131314 !important;color:#ececf1 !important;}",
    ".em-h1{color:#ececf1 !important;}",
    ".em-lead{color:#c4c4cc !important;}",
    ".em-lead strong{color:#ececf1 !important;}",
    ".em-k{color:#7a7a84 !important;border-color:#2a2a2f !important;}",
    ".em-v{color:#ececf1 !important;border-color:#2a2a2f !important;}",
    ".em-btn-cell{background-color:#f4f4f5 !important;}",
    ".em-btn{background-color:#f4f4f5 !important;color:#0a0a0f !important;}",
    ".em-fb{color:#7a7a84 !important;}",
    ".em-link{color:#a1a1aa !important;}",
    ".em-foot{color:#7a7a84 !important;border-top-color:#2a2a2f !important;}",
    "}",
  ].join("");

  const html = [
    "<!doctype html>",
    '<html lang="en">',
    "<head>",
    '<meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width,initial-scale=1">',
    '<meta name="color-scheme" content="light dark">',
    '<meta name="supported-color-schemes" content="light dark">',
    `<title>${escapedSubject}</title>`,
    `<style>${headStyles}</style>`,
    "</head>",
    `<body class="em-bg" style="margin:0;padding:0;background-color:#ffffff;color:#0a0a0f;font-family:${FONT_STACK};-webkit-text-size-adjust:100%;-ms-text-size-adjust:100%;">`,
    `<div style="display:none;max-height:0;overflow:hidden;opacity:0;mso-hide:all;font-size:1px;line-height:1px;color:#ffffff;">Join as ${escapedRole} &middot; expires ${escapedExpiry}&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;</div>`,
    `<table class="em-bg" role="presentation" width="100%" cellspacing="0" cellpadding="0" border="0" style="width:100%;border-collapse:collapse;background-color:#ffffff;">`,
    '<tr><td align="center" style="padding:28px 24px 36px;">',
    '<table role="presentation" width="100%" cellspacing="0" cellpadding="0" border="0" style="max-width:480px;width:100%;border-collapse:collapse;text-align:left;">',
    "<tr><td>",
    `<img src="${BRAND_ICON_URL}" alt="Sikemux" width="40" height="40" style="display:block;width:40px;height:40px;border:0;border-radius:9px;">`,
    `<h1 class="em-h1" style="margin:28px 0 10px;font-family:${FONT_STACK};font-size:24px;line-height:30px;font-weight:600;letter-spacing:-0.022em;color:#0a0a0f;">Join ${escapedOrg} on Sikemux</h1>`,
    `<p class="em-lead" style="margin:0 0 24px;font-family:${FONT_STACK};font-size:15px;line-height:24px;color:#3f3f46;">${introHtml}</p>`,
    `<table role="presentation" width="100%" cellspacing="0" cellpadding="0" border="0" style="width:100%;border-collapse:collapse;margin:0 0 28px;font-family:${FONT_STACK};font-size:14px;line-height:20px;">`,
    `<tr><td class="em-k" width="120" style="width:120px;padding:11px 0;border-top:1px solid #ececef;color:#8a8a94;vertical-align:top;">Role</td><td class="em-v" style="padding:11px 0;border-top:1px solid #ececef;color:#0a0a0f;font-weight:500;vertical-align:top;">${escapedRole}</td></tr>`,
    `<tr><td class="em-k" width="120" style="width:120px;padding:11px 0;border-top:1px solid #ececef;border-bottom:1px solid #ececef;color:#8a8a94;vertical-align:top;">Expires</td><td class="em-v" style="padding:11px 0;border-top:1px solid #ececef;border-bottom:1px solid #ececef;color:#0a0a0f;font-weight:500;vertical-align:top;">${escapedExpiry}</td></tr>`,
    "</table>",
    '<table role="presentation" cellspacing="0" cellpadding="0" border="0" style="margin:0;border-collapse:collapse;">',
    `<tr><td class="em-btn-cell" align="center" style="border-radius:8px;background-color:#0a0a0f;"><a class="em-btn" href="${escapedLink}" style="display:inline-block;padding:12px 22px;border-radius:8px;background-color:#0a0a0f;color:#ffffff;font-family:${FONT_STACK};font-size:14px;line-height:20px;font-weight:600;text-decoration:none;">Accept invite</a></td></tr>`,
    "</table>",
    `<p class="em-fb" style="margin:24px 0 0;font-family:${FONT_STACK};font-size:12.5px;line-height:19px;color:#8a8a94;">Button not working? Open this link:<br><a class="em-link" href="${escapedLink}" style="display:inline-block;margin-top:4px;font-family:${MONO_STACK};font-size:12px;line-height:18px;color:#52525b;text-decoration:none;word-break:break-all;">${escapedLink}</a></p>`,
    `<p class="em-foot" style="margin:36px 0 0;padding-top:18px;border-top:1px solid #ececef;font-family:${FONT_STACK};font-size:12px;line-height:18px;color:#8a8a94;">This invite works once. Not expecting it? Ignore this email and nothing changes.</p>`,
    "</td></tr>",
    "</table>",
    "</td></tr>",
    "</table>",
    "</body>",
    "</html>",
  ].join("");

  const text = [
    `Join ${safeOrg} on Sikemux`,
    "",
    introText,
    "",
    `Role: ${roleLabel}`,
    `Expires: ${expiry}`,
    "",
    `Accept invite: ${invite.link}`,
    "",
    "This invite works once. Not expecting it? Ignore this email and nothing changes.",
  ].join("\n");

  return { subject, html, text };
}

export function resendInviteMailer(
  settings: ResendSettings,
  fetchImpl: typeof fetch = fetch,
): InviteMailer {
  return {
    async sendInvite(invite, log) {
      const { subject, html, text } = buildInviteEmail(invite);
      let response: Response;
      try {
        response = await fetchImpl(RESEND_API_URL, {
          method: "POST",
          headers: {
            authorization: `Bearer ${settings.apiKey}`,
            "content-type": "application/json",
            "user-agent": "sikemux-api",
          },
          body: JSON.stringify({
            from: settings.from,
            to: [invite.to],
            subject,
            html,
            text,
          }),
          signal: AbortSignal.timeout(SEND_TIMEOUT_MS),
        });
      } catch (error) {
        log.warn(
          { err: error },
          "Resend could not be reached to send an org invite",
        );
        throw new ApiFailure(
          503,
          "unavailable",
          "Could not reach the email service to send the invite. Try again or use an invite link instead.",
        );
      }

      if (!response.ok) {
        const detail = (await response.json().catch(() => null)) as {
          name?: string;
        } | null;
        log.warn(
          { status: response.status, resendError: detail?.name ?? null },
          "Resend refused to send an org invite",
        );
        if (response.status === 422)
          throw new ApiFailure(
            400,
            "bad_request",
            "That email address could not be emailed. Check the address and try again.",
          );
        throw new ApiFailure(
          503,
          "unavailable",
          "Could not send the invite email right now. Try again or use an invite link instead.",
        );
      }
    },
  };
}

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

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function formatExpiry(date: Date): string {
  return new Intl.DateTimeFormat("en-US", {
    dateStyle: "long",
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
  const subject = `You're invited to join ${safeOrg} on Sikemux`;

  const introText = safeInviter
    ? `${safeInviter} invited you to join ${safeOrg} on Sikemux as ${roleLabel}.`
    : `You've been invited to join ${safeOrg} on Sikemux as ${roleLabel}.`;

  const introHtml = safeInviter
    ? `<strong>${escapeHtml(safeInviter)}</strong> invited you to join <strong>${escapeHtml(safeOrg)}</strong> on Sikemux as <strong>${escapeHtml(roleLabel)}</strong>.`
    : `You've been invited to join <strong>${escapeHtml(safeOrg)}</strong> on Sikemux as <strong>${escapeHtml(roleLabel)}</strong>.`;

  const html = [
    "<!doctype html>",
    '<html lang="en">',
    "<body style=\"margin:0;padding:32px 16px;background:#f4f4f6;color:#111114;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;line-height:1.5;\">",
    '<table role="presentation" width="100%" cellspacing="0" cellpadding="0" style="max-width:520px;margin:0 auto;background:#ffffff;border:1px solid #e4e4e7;border-radius:12px;">',
    '<tr><td style="padding:32px 28px;">',
    '<div style="font-size:13px;font-weight:600;letter-spacing:0.06em;text-transform:uppercase;color:#71717a;margin-bottom:12px;">Sikemux</div>',
    `<h1 style="margin:0 0 12px;font-size:22px;font-weight:600;letter-spacing:-0.02em;color:#09090b;">Join ${escapeHtml(safeOrg)}</h1>`,
    `<p style="margin:0 0 24px;font-size:15px;color:#3f3f46;">${introHtml}</p>`,
    `<table role="presentation" cellspacing="0" cellpadding="0" style="margin:0 0 24px;"><tr><td style="border-radius:8px;background:#7c3aed;"><a href="${escapeHtml(invite.link)}" style="display:inline-block;padding:11px 20px;font-size:14px;font-weight:600;color:#ffffff;text-decoration:none;">Accept invite</a></td></tr></table>`,
    `<p style="margin:0 0 16px;font-size:13px;color:#52525b;">This one-time invite expires on <strong>${escapeHtml(expiry)}</strong>.</p>`,
    `<p style="margin:0;padding-top:16px;border-top:1px solid #e4e4e7;font-size:12px;color:#71717a;word-break:break-all;">Or open this link in your browser:<br><a href="${escapeHtml(invite.link)}" style="color:#6d28d9;text-decoration:underline;">${escapeHtml(invite.link)}</a></p>`,
    "</td></tr>",
    "</table>",
    "</body>",
    "</html>",
  ].join("");

  const text = [
    introText,
    "",
    `Accept invite: ${invite.link}`,
    "",
    `Role: ${roleLabel}`,
    `Expires: ${expiry}`,
    "",
    "If you were not expecting this invite, you can ignore this email.",
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

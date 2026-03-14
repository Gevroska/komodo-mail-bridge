import html
import json
import os
import smtplib
from datetime import datetime, timezone
from email.message import EmailMessage
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from flask import Flask, jsonify, request

app = Flask(__name__)

SMTP_HOST = os.getenv("SMTP_HOST", "postfix")
SMTP_PORT = int(os.getenv("SMTP_PORT", "25"))
FROM_ADDR = os.getenv("FROM_ADDR", "komodo@localhost")
DEFAULT_TO = os.getenv("TO_ADDR", "")
SUBJECT_PREFIX = os.getenv("SUBJECT_PREFIX", "[Komodo]")
DISPLAY_TIMEZONE = os.getenv("DISPLAY_TIMEZONE", "UTC")


def get_timezone():
    try:
        return ZoneInfo(DISPLAY_TIMEZONE)
    except ZoneInfoNotFoundError:
        return timezone.utc


def format_ts(ts):
    try:
        if not ts:
            return "-"
        # Komodo timestamps appear to be in milliseconds
        dt = datetime.fromtimestamp(ts / 1000, tz=timezone.utc).astimezone(get_timezone())
        return dt.strftime("%Y-%m-%d %H:%M:%S %Z")
    except Exception:
        return str(ts)


def normalize_dict(value):
    return value if isinstance(value, dict) else {}


def esc(value):
    return html.escape(str(value if value is not None else "-"))


def badge_colors(level, resolved):
    if resolved:
        return {
            "bg": "#e8f5e9",
            "fg": "#1b5e20",
            "border": "#a5d6a7",
        }

    level_upper = str(level).upper()
    if level_upper in {"CRITICAL", "ERROR", "FAIL", "FAILED"}:
        return {
            "bg": "#ffebee",
            "fg": "#b71c1c",
            "border": "#ef9a9a",
        }
    if level_upper in {"WARNING", "WARN"}:
        return {
            "bg": "#fff8e1",
            "fg": "#8d6e00",
            "border": "#ffe082",
        }
    if level_upper in {"OK", "INFO"}:
        return {
            "bg": "#e3f2fd",
            "fg": "#0d47a1",
            "border": "#90caf9",
        }

    return {
        "bg": "#f5f5f5",
        "fg": "#424242",
        "border": "#d0d0d0",
    }


def build_subject(alert_type, level, alert_name, resolved):
    status = "RESOLVED" if resolved else "ACTIVE"
    parts = [SUBJECT_PREFIX, status, alert_type, level]
    if alert_name and alert_name != "-":
        parts.append(alert_name)
    return " ".join(parts)


def build_text_body(
    status,
    level,
    alert_type,
    alert_name,
    alert_id,
    target_type,
    target_id,
    triggered,
    resolved_at,
    raw_payload_pretty,
):
    return f"""Komodo Alert

Status      : {status}
Level       : {level}
Alert Type  : {alert_type}
Name        : {alert_name}
Alert ID    : {alert_id}

Target Type : {target_type}
Target ID   : {target_id}

Triggered   : {triggered}
Resolved    : {resolved_at}

Raw Payload
-----------
{raw_payload_pretty}
"""


def build_html_body(
    status,
    level,
    alert_type,
    alert_name,
    alert_id,
    target_type,
    target_id,
    triggered,
    resolved_at,
    raw_payload_pretty,
    resolved,
):
    colors = badge_colors(level, resolved)
    status_label = "Resolved" if resolved else "Active"
    status_badge = (
        f"<span style=\"display:inline-block;padding:4px 10px;border-radius:999px;"
        f"border:1px solid {colors['border']};background:{colors['bg']};color:{colors['fg']};"
        "font-size:12px;font-weight:700;text-transform:uppercase;letter-spacing:0.03em;\">"
        f"{esc(status)}"
        "</span>"
    )

    return f"""\
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Komodo Alert</title>
</head>
<body style="margin:0;padding:0;background:#f4f6f8;font-family:Arial,Helvetica,sans-serif;color:#1f2937;">
  <div style="max-width:720px;margin:0 auto;padding:24px 16px;">
    <div style="background:#ffffff;border:1px solid #e5e7eb;border-radius:14px;overflow:hidden;box-shadow:0 1px 3px rgba(0,0,0,0.06);">
      <div style="padding:20px 24px;border-bottom:1px solid #e5e7eb;background:#111827;">
        <div style="font-size:12px;line-height:1;color:#cbd5e1;letter-spacing:0.08em;text-transform:uppercase;margin-bottom:10px;">
          Komodo Notification
        </div>
        <div style="font-size:26px;font-weight:700;line-height:1.2;color:#ffffff;">
          {esc(alert_type)}
        </div>
        <div style="margin-top:12px;">
          <span style="display:inline-block;padding:6px 10px;border-radius:999px;border:1px solid {colors['border']};background:{colors['bg']};color:{colors['fg']};font-size:12px;font-weight:700;text-transform:uppercase;letter-spacing:0.03em;">
            {esc(status_label)}
          </span>
          <span style="display:inline-block;margin-left:8px;padding:6px 10px;border-radius:999px;border:1px solid #dbeafe;background:#eff6ff;color:#1d4ed8;font-size:12px;font-weight:700;text-transform:uppercase;letter-spacing:0.03em;">
            {esc(level)}
          </span>
        </div>
      </div>

      <div style="padding:24px;">
        <div style="font-size:18px;font-weight:700;color:#111827;margin-bottom:6px;">
          {esc(alert_name)}
        </div>
        <div style="font-size:14px;color:#6b7280;margin-bottom:20px;">
          Automated alert received from Komodo.
        </div>

        <table role="presentation" cellpadding="0" cellspacing="0" border="0" width="100%" style="border-collapse:collapse;">
          <tr>
            <td style="padding:0 0 18px 0;">
              <table role="presentation" cellpadding="0" cellspacing="0" border="0" width="100%" style="border-collapse:separate;border-spacing:0;">
                <tr>
                  <td colspan="2" style="padding:0 0 10px 0;font-size:14px;font-weight:700;color:#111827;">
                    Alert Details
                  </td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Status</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;font-weight:600;">{status_badge}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Level</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;">{esc(level)}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Alert Type</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;">{esc(alert_type)}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Name</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;">{esc(alert_name)}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Alert ID</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;font-family:Consolas,Monaco,monospace;">{esc(alert_id)}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Target Type</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;">{esc(target_type)}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Target ID</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;font-family:Consolas,Monaco,monospace;">{esc(target_id)}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Triggered</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;color:#111827;font-size:13px;">{esc(triggered)}</td>
                </tr>
                <tr>
                  <td style="width:180px;padding:10px 12px;border-top:1px solid #e5e7eb;border-bottom:1px solid #e5e7eb;color:#6b7280;font-size:13px;">Resolved</td>
                  <td style="padding:10px 12px;border-top:1px solid #e5e7eb;border-bottom:1px solid #e5e7eb;color:#111827;font-size:13px;">{esc(resolved_at)}</td>
                </tr>
              </table>
            </td>
          </tr>
        </table>

        <details style="margin-top:8px;">
          <summary style="cursor:pointer;font-size:14px;font-weight:700;color:#111827;">
            Raw Payload
          </summary>
          <div style="margin-top:8px;padding:14px 16px;background:#0f172a;color:#e5e7eb;border-radius:10px;font-family:Consolas,Monaco,monospace;font-size:12px;line-height:1.5;white-space:pre-wrap;word-break:break-word;">
{esc(raw_payload_pretty)}
          </div>
        </details>
      </div>
    </div>
  </div>
</body>
</html>
"""


@app.get("/health")
def health():
    return {"ok": True}, 200


@app.post("/komodo")
def komodo():
    payload = request.get_json(silent=True) or {}

    level = payload.get("level", "INFO")
    resolved = bool(payload.get("resolved", False))
    status = "RESOLVED" if resolved else "ACTIVE"
    triggered = format_ts(payload.get("ts"))
    resolved_at = format_ts(payload.get("resolved_ts"))

    target = normalize_dict(payload.get("target"))
    target_type = target.get("type", "-")
    target_id = target.get("id", "-")

    data = normalize_dict(payload.get("data"))
    alert_type = data.get("type", "Unknown")

    inner = normalize_dict(data.get("data"))
    alert_name = inner.get("name") or inner.get("id") or data.get("name") or "-"
    alert_id = inner.get("id") or data.get("id") or "-"

    raw_payload_pretty = json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True)

    if not DEFAULT_TO:
        return jsonify({"error": "TO_ADDR is empty"}), 400

    subject = build_subject(alert_type, level, alert_name, resolved)
    text_body = build_text_body(
        status,
        level,
        alert_type,
        alert_name,
        alert_id,
        target_type,
        target_id,
        triggered,
        resolved_at,
        raw_payload_pretty,
    )
    html_body = build_html_body(
        status,
        level,
        alert_type,
        alert_name,
        alert_id,
        target_type,
        target_id,
        triggered,
        resolved_at,
        raw_payload_pretty,
        resolved,
    )

    msg = EmailMessage()
    msg["From"] = FROM_ADDR
    msg["To"] = DEFAULT_TO
    msg["Subject"] = subject
    msg.set_content(text_body)
    msg.add_alternative(html_body, subtype="html")

    try:
        with smtplib.SMTP(SMTP_HOST, SMTP_PORT, timeout=15) as smtp:
            smtp.send_message(msg)
    except Exception as e:
        return jsonify({"error": str(e)}), 500

    return {"ok": True}, 200


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=8000)

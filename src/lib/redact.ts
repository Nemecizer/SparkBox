// Hides anything that looks like a secret. Same rules as redact() in
// src-tauri/src/live/audit.rs, which applies them before the activity log is written;
// Inspect's "Hide secrets" and Fleet output use them here.

export const REDACTED = "‹redacted›";

/** Environment variable names whose values are secrets. */
export const SECRET_NAME = /(TOKEN|KEY|SECRET|PASSWORD|PASSWD|PASS|CREDENTIAL)$/i;

// A private key from its BEGIN line through its END line, or to the end when cut off.
// PGP's ends in "PRIVATE KEY BLOCK".
const PEM = /-----BEGIN ([A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?)-----([\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|[\s\S]*)/g;
// curl -u user:password: the user stays (without a password curl asks, so nothing to hide).
const CURL_USER = /\b(curl\b[^\n;|&]*?\s(?:-u|--user)(?:\s+|=))('[^']*'|"[^"]*"|[^\s'"]+)/g;
// --db-password=…, --proxy-password …, --client-secret …, --access-token …
const NAMED_SECRET_FLAG = /(--(?:[a-z0-9]+-)+(?:password|passwd|secret|token))([= ])('[^']*'|"[^"]*"|[^\s'"]+)/g;
const AWS = /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/g;
const SLACK = /(https?:\/\/hooks\.slack\.com\/services\/)[A-Za-z0-9/_-]+/g;
// Discord, Teams (Office 365 connectors) and Google Chat webhooks, and Healthchecks ping URLs: the path is the secret.
const WEBHOOK = /(https?:\/\/(?:(?:(?:canary|ptb)\.)?discord(?:app)?\.com\/api\/webhooks\/|[A-Za-z0-9.-]+\.webhook\.office\.com\/webhookb2\/|outlook\.office(?:365)?\.com\/webhook\/|chat\.googleapis\.com\/v1\/spaces\/|hc-ping\.com\/))[^\s'"]+/g;
// Teams workflow (Power Automate) URLs carry a signature.
const SIGNED = /([?&]sig=)[^&\s'"]+/g;
// INI and TOML style, with spaces around the = (~/.aws/credentials).
const INI = /\b([a-z0-9_.-]*(?:secret[a-z0-9_]*|key|token|password|passwd|pass))(\s+=\s*|\s*=\s+)('[^']*'|"[^"]*"|[^\s'"]+)/gi;
const NTFY = /\btk_[A-Za-z0-9]{24,}/g;
const JSON_SECRET = /("[a-z0-9_-]*(?:api_?key|token|secret|password|authorization)"\s*:\s*")((?:[^"\\]|\\.)*)"/gi;
// A Python dict as a log prints it: vLLM's "non-default args: {'api_key': ['…']}".
const PY_SECRET = /('[a-z0-9_-]*(?:api_?key|token|secret|password|authorization)'\s*:\s*\[?\s*')((?:[^'\\]|\\.)*)'/gi;
const HEADER = /\b(authorization|x-api-key)(\s*:\s*)([^'"\r\n]+)/gi;
const ASSIGNMENT = /\b([A-Z0-9_]*(?:TOKEN|KEY|SECRET|PASSWORD|PASSWD|PASS))=('[^']*'|"[^"]*"|[^\s'"]+)/gi;
const BEARER = /\bBearer\s+[^\s'"]+/gi;
// "Token <value>" is how Firewalla's cloud is sent its key, here without the header's name.
// Case-sensitive and long, so "Token limit reached" stays.
const MSP_TOKEN = /\bToken\s+[A-Za-z0-9._~+/=-]{20,}/g;
const SECRET_FLAG = /(--(?:password|passwd|token|api-key|apikey|secret|key))([= ])('[^']*'|"[^"]*"|[^\s'"]+)/gi;
// mysql's -p<password> (attached) and `docker login -p <password>`; `ssh -p 22` and `docker run -p` stay.
const MYSQL_P = /\b((?:mysql|mysqldump|mysqladmin|mariadb)\b[^\n;|&]*?\s)-p([^\s'"]+)/g;
const DOCKER_P = /\b(docker\s+login\b[^\n;|&]*?\s)-p(\s+|=)?([^\s'"]+)/g;
const URL_PASSWORD = /(\b[a-z][a-z0-9+.-]*:\/\/[^:/@\s]+:)[^@\s]+@/g;
// hf_ + 34 letters and digits; short lowercase words like the hf_transfer package stay.
const HF = /\bhf_[A-Za-z0-9]{8,}/g;
const SK = /\bsk-[A-Za-z0-9_-]{8,}/g;
// A load's API key (sbk- and 32 hex). The masked form the app shows has no ASCII after sbk-.
const SBK = /\bsbk-[A-Za-z0-9_-]{8,}/g;
const GITHUB = /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})/g;

function looksLikeToken(s: string): boolean {
  const body = s.slice(3);
  return body.length >= 30 || /[0-9A-Z]/.test(body);
}

/**
 * Hides anything that looks like a secret: private key blocks, KEY=value pairs, secret-looking
 * JSON fields, Authorization and X-Api-Key headers, bearer and Firewalla tokens, --password/--token/
 * --api-key/--secret/--key values and --anything-password/-secret/-token ones, curl -u's
 * password, mysql's -p<password> and `docker login -p`, passwords in URLs, Slack, Discord, Teams
 * and Google Chat webhook paths, Healthchecks ping URLs, `sig=` signatures, INI-style
 * `aws_secret_access_key = …`, PGP private key blocks, Python dict fields as a log prints them
 * (`{'api_key': ['…']}`), and Hugging Face, OpenAI/Anthropic, a load's own (sbk-…), GitHub, AWS
 * (AKIA…) and ntfy (tk_…) tokens.
 * src/lib/redact_cases.json is checked on both sides.
 */
export function redact(text: string): string {
  return text
    .replace(PEM, (_m, kind: string, rest: string) =>
      rest.trimEnd().endsWith("-----") && rest.includes("-----END ") ? `-----BEGIN ${kind}-----\n${REDACTED}\n-----END ${kind}-----` : `-----BEGIN ${kind}-----\n${REDACTED}`)
    .replace(CURL_USER, (m, head: string, value: string) => {
      const quote = value[0] === "'" || value[0] === '"' ? value[0] : "";
      const inner = value.replace(/^['"]|['"]$/g, "");
      const at = inner.indexOf(":");
      return at < 0 ? m : `${head}${quote}${inner.slice(0, at)}:${REDACTED}${quote}`;
    })
    .replace(NAMED_SECRET_FLAG, (_m, flag: string, sep: string) => `${flag}${sep}${REDACTED}`)
    .replace(JSON_SECRET, (_m, head: string) => `${head}${REDACTED}"`)
    .replace(PY_SECRET, (_m, head: string) => `${head}${REDACTED}'`)
    .replace(HEADER, (_m, name: string, sep: string) => `${name}${sep}${REDACTED}`)
    .replace(ASSIGNMENT, (_m, name: string) => `${name}=${REDACTED}`)
    .replace(INI, (_m, name: string, sep: string) => `${name}${sep}${REDACTED}`)
    .replace(BEARER, `Bearer ${REDACTED}`)
    .replace(MSP_TOKEN, `Token ${REDACTED}`)
    .replace(SECRET_FLAG, (_m, flag: string, sep: string) => `${flag}${sep}${REDACTED}`)
    .replace(MYSQL_P, (_m, head: string) => `${head}-p${REDACTED}`)
    .replace(DOCKER_P, (_m, head: string, sep: string | undefined) => `${head}-p${sep ?? ""}${REDACTED}`)
    .replace(URL_PASSWORD, (_m, head: string) => `${head}${REDACTED}@`)
    .replace(HF, (m) => (looksLikeToken(m) ? REDACTED : m))
    .replace(SK, REDACTED)
    .replace(SBK, REDACTED)
    .replace(GITHUB, REDACTED)
    .replace(AWS, REDACTED)
    .replace(SLACK, (_m, head: string) => `${head}${REDACTED}`)
    .replace(WEBHOOK, (_m, head: string) => `${head}${REDACTED}`)
    .replace(SIGNED, (_m, head: string) => `${head}${REDACTED}`)
    .replace(NTFY, REDACTED);
}

/** "NAME=value" with the value hidden when the name says it's a secret. */
export function redactEnv(entry: string): string {
  const eq = entry.indexOf("=");
  if (eq < 0) return entry;
  const name = entry.slice(0, eq);
  return SECRET_NAME.test(name) ? `${name}=${REDACTED}` : redact(entry);
}

/** Redacts every string inside a JSON value (docker inspect output, for copying). */
export function redactDeep<T>(value: T): T {
  if (typeof value === "string") return redact(value) as T;
  if (Array.isArray(value)) return value.map(redactDeep) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([k, v]) => [k, redactDeep(v)])) as T;
  }
  return value;
}

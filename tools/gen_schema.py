#!/usr/bin/env python3
"""Regenerates src/schema.rs from OpenSSH's own sources.

The keyword list, value types and allowed enum values come from readconf.c (the
parser itself, so they cannot drift from what ssh accepts); the descriptions and
defaults come from the ssh_config.5 man page.

    python3 tools/gen_schema.py            # downloads from openssh-portable
    python3 tools/gen_schema.py /path/src  # uses a local checkout
"""

import json
import os
import re
import sys
import urllib.request

BASE = "https://raw.githubusercontent.com/openssh/openssh-portable/master"

# Options worth surfacing before the long tail, based on what people actually
# put in ~/.ssh/config: connection basics, keys, jump hosts, forwards,
# keepalives and multiplexing.
COMMON = {
    "HostName", "User", "Port", "IdentityFile", "IdentitiesOnly", "IdentityAgent",
    "ProxyJump", "ProxyCommand", "LocalForward", "RemoteForward", "DynamicForward",
    "ForwardAgent", "AddKeysToAgent", "ForwardX11", "ForwardX11Trusted",
    "ServerAliveInterval", "ServerAliveCountMax", "TCPKeepAlive", "ConnectTimeout",
    "ControlMaster", "ControlPath", "ControlPersist", "Compression",
    "StrictHostKeyChecking", "UserKnownHostsFile", "CheckHostIP",
    "PreferredAuthentications", "PubkeyAuthentication", "PasswordAuthentication",
    "BatchMode", "RequestTTY", "RemoteCommand", "LocalCommand", "PermitLocalCommand",
    "SetEnv", "SendEnv", "LogLevel", "ExitOnForwardFailure", "Include",
}

# Extra search terms, so an option can be found by what it does rather than by
# its name. These are the words people actually search for: "socks", "tunnel",
# "bastion", "keepalive", "reuse". Descriptions are truncated to one sentence,
# which loses terms like SOCKS, so searching leans on these.
ALIASES = {
    "LocalForward": "tunnel port forward local -L",
    "RemoteForward": "tunnel port forward remote reverse -R",
    "DynamicForward": "socks proxy tunnel -D",
    "ProxyJump": "bastion jump host via through -J",
    "ProxyCommand": "bastion jump proxy netcat",
    "IdentityFile": "key private key pem identity",
    "IdentityAgent": "agent socket 1password keychain",
    "IdentitiesOnly": "key too many authentication failures",
    "AddKeysToAgent": "agent keychain passphrase",
    "ForwardAgent": "agent forwarding",
    "ForwardX11": "x11 gui display graphical",
    "ServerAliveInterval": "keepalive timeout idle drop disconnect frozen",
    "ServerAliveCountMax": "keepalive timeout drop disconnect",
    "TCPKeepAlive": "keepalive timeout",
    "ConnectTimeout": "timeout slow",
    "ControlMaster": "multiplex reuse connection sharing faster persistent",
    "ControlPath": "multiplex reuse socket",
    "ControlPersist": "multiplex reuse background keep open",
    "Compression": "slow link bandwidth",
    "StrictHostKeyChecking": "host key fingerprint prompt changed warning",
    "UserKnownHostsFile": "known hosts fingerprint",
    "CheckHostIP": "known hosts ip",
    "RequestTTY": "tty terminal interactive pseudo",
    "RemoteCommand": "run command on connect tmux",
    "LocalCommand": "run command locally hook",
    "PermitLocalCommand": "run command locally hook",
    "SetEnv": "environment variable env",
    "SendEnv": "environment variable env locale",
    "BatchMode": "script non-interactive no password prompt automation",
    "PasswordAuthentication": "password login auth",
    "PubkeyAuthentication": "key login auth publickey",
    "PreferredAuthentications": "auth order login",
    "KbdInteractiveAuthentication": "auth 2fa mfa otp keyboard",
    "LogLevel": "debug verbose quiet logging",
    "ExitOnForwardFailure": "forward fail tunnel error",
    "Ciphers": "crypto algorithm legacy old server cipher",
    "MACs": "crypto algorithm legacy old server",
    "KexAlgorithms": "crypto algorithm legacy old server key exchange",
    "HostKeyAlgorithms": "crypto algorithm legacy old server",
    "PubkeyAcceptedAlgorithms": "crypto algorithm legacy old server rsa-sha2 ssh-rsa",
    "Include": "include another file",
    "CanonicalizeHostname": "short name domain suffix expand",
    "VisualHostKey": "fingerprint ascii art",
    "HostName": "address ip real host",
    "User": "username login",
    "Port": "port number",
    "Tunnel": "vpn tun tap layer 2 device",
    "GatewayPorts": "bind all interfaces expose forward",
}

# Options that are meaningful more than once in a single host block.
REPEATABLE = {
    "IdentityFile", "CertificateFile", "LocalForward", "RemoteForward",
    "DynamicForward", "SendEnv", "SetEnv", "Include", "PermitRemoteOpen",
    "PermitLocalOpen", "KnownHostsCommand", "CanonicalDomains",
}

# parse_* label in readconf.c -> our value editor.
KIND_MAP = {
    "parse_flag": "Flag",
    "enum": "Choice",
    "parse_int": "Int",
    "parse_time": "Time",
    "parse_command": "Command",
    "parse_char_array": "Algorithms",
    "parse_pubkey_algos": "Algorithms",
    "parse_string": "Text",
}

PATH_LIKE = re.compile(r"(File|Path|Agent|KnownHosts)")


def source(name, local):
    if local:
        return open(os.path.join(local, name)).read()
    with urllib.request.urlopen(f"{BASE}/{name}") as response:
        return response.read().decode("utf-8", "replace")


def multistate_tables(src):
    tables = {}
    for m in re.finditer(r"static const struct multistate (multistate_\w+)\[\] = \{(.*?)\n\};", src, re.S):
        values = [v for v in re.findall(r'\{\s*"([^"]+)"', m.group(2)) if v not in ("true", "false")]
        tables[m.group(1)] = values
    return tables


def keyword_opcodes(src):
    block = re.search(r"\} keywords\[\] = \{(.*?)\n\};", src, re.S).group(1)
    return {m.group(1): m.group(2) for m in re.finditer(r'\{\s*"([^"]+)",\s*(o\w+)', block)}


def opcode_kinds(src, tables):
    switch = src[src.index("switch (opcode) {"):]
    cases = list(re.finditer(r"\n\tcase (o\w+):", switch))
    kinds = {}
    for i, case in enumerate(cases):
        opcodes = [case.group(1)]
        start, j = case.end(), i + 1
        # `case oA:` immediately followed by `case oB:` share one body
        while j < len(cases) and switch[start:cases[j].start()].strip() == "":
            opcodes.append(cases[j].group(1))
            start, j = cases[j].end(), j + 1
        body = switch[start:cases[j].start() if j < len(cases) else len(switch)]

        hits = []
        for m in re.finditer(r"multistate_ptr = (multistate_\w+);", body):
            hits.append((m.start(), ("enum", tables.get(m.group(1), []))))
        for m in re.finditer(r"parse_multistate_value\([^;]*?(multistate_\w+)\)", body, re.S):
            hits.append((m.start(), ("enum", tables.get(m.group(1), []))))
        for label in KIND_MAP:
            if label == "enum":
                continue
            m = re.search(r"(?:goto %s;|^ ?%s:)" % (label, label), body, re.M)
            if m:
                hits.append((m.start(), (label, [])))
        if hits:
            hits.sort(key=lambda h: h[0])
            for opcode in opcodes:
                kinds[opcode] = hits[0][1]
    return kinds


def man_entries(man):
    entries = {}
    for part in re.split(r"\n\.It Cm ", man)[1:]:
        head, _, rest = part.partition("\n")
        name = head.split()[0].strip()
        entries[name.lower()] = (name, rest.split("\n.It Cm ")[0])
    return entries


def clean(text):
    out = []
    for line in text.split("\n"):
        s = line.strip()
        if not s.startswith("."):
            out.append(s)
            continue
        tokens = s[1:].split()
        if not tokens or tokens[0] in ("Pp", "Bl", "El", "It", "Sh", "Bd", "Ed", "Nm", "Sx"):
            continue
        if tokens[0] == "Xr" and len(tokens) > 1:
            out.append(tokens[1])
            continue
        # Drop the mdoc macro name, keep its arguments.
        out.append(" ".join(t for t in tokens[1:] if not re.fullmatch(r"[A-Z][a-z]", t)))
    text = re.sub(r"\s+", " ", " ".join(out)).replace("\\&", "").replace("\\-", "-")
    return re.sub(r"\s+([,.;])", r"\1", text).strip()


def first_sentence(text, limit=120):
    text = clean(text)
    m = re.match(r"(.{20,}?\.)(\s|$)", text)
    sentence = m.group(1) if m else text
    if len(sentence) > limit:
        sentence = sentence[:limit].rsplit(" ", 1)[0] + "…"
    return sentence


def default_of(raw):
    m = re.search(r"\.Cm ([\w-]+)\s*,?\s*\n\(the default\)", raw)
    if m:
        return m.group(1)
    m = re.search(r"[Tt]he default is\s*\n?\.(?:Cm|Ar|Dv|Pa)\s+([^\s,.]+)", raw)
    if m:
        return m.group(1).strip('"')
    m = re.search(r"[Tt]he default is ([\w./~-]{1,24})[.,]", clean(raw))
    return m.group(1) if m else None


def rust_string(text):
    return text.replace("\\", "\\\\").replace('"', '\\"')


def main():
    local = sys.argv[1] if len(sys.argv) > 1 else None
    readconf = source("readconf.c", local)
    man = source("ssh_config.5", local)

    tables = multistate_tables(readconf)
    kinds = opcode_kinds(readconf, tables)
    entries = man_entries(man)

    rows = []
    for lower, opcode in sorted(keyword_opcodes(readconf).items()):
        if opcode in ("oDeprecated", "oUnsupported", "oIgnore", "oIgnoredUnknownOption"):
            continue
        if lower in ("host", "match"):  # block headers, not options
            continue
        if lower not in entries:  # undocumented: don't guess at its meaning
            continue
        name, body = entries[lower]
        label, values = kinds.get(opcode, ("parse_string", []))
        kind = KIND_MAP[label]
        if kind == "Text" and PATH_LIKE.search(name):
            kind = "Path"
        if kind == "Flag":
            values = ["yes", "no"]
        rows.append({
            "name": name,
            "kind": kind,
            "values": values,
            "desc": first_sentence(body),
            "default": default_of(body),
            "aliases": ALIASES.get(name, ""),
            "common": name in COMMON,
            "repeatable": name in REPEATABLE,
        })

    lines = [
        "//! The ssh_config keyword catalogue: every option ssh accepts, with the",
        "//! value type, allowed values, default and a one-line description.",
        "//!",
        "//! @generated by tools/gen_schema.py from OpenSSH's readconf.c and",
        "//! ssh_config.5 — do not edit by hand; re-run the generator instead.",
        "",
        "/// How a value should be edited.",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub enum Kind {",
        "    /// yes / no.",
        "    Flag,",
        "    /// One of a fixed set of words.",
        "    Choice,",
        "    /// A plain integer.",
        "    Int,",
        "    /// A time, either seconds or a suffixed form like `10m`.",
        "    Time,",
        "    /// A filesystem path.",
        "    Path,",
        "    /// A shell command line.",
        "    Command,",
        "    /// A comma-separated algorithm list.",
        "    Algorithms,",
        "    /// Anything else.",
        "    Text,",
        "}",
        "",
        "pub struct Keyword {",
        "    pub name: &'static str,",
        "    pub kind: Kind,",
        "    /// Allowed values for [`Kind::Flag`] and [`Kind::Choice`], else empty.",
        "    pub values: &'static [&'static str],",
        "    pub desc: &'static str,",
        "    /// Extra words this option can be searched by.",
        "    pub aliases: &'static str,",
        "    pub default: Option<&'static str>,",
        "    /// Shown at the top of the option picker.",
        "    pub common: bool,",
        "    /// Meaningful more than once in one host block.",
        "    pub repeatable: bool,",
        "}",
        "",
        f"/// Every documented ssh_config keyword ({len(rows)} of them), alphabetically.",
        "pub static KEYWORDS: &[Keyword] = &[",
    ]
    for row in rows:
        values = ", ".join(f'"{v}"' for v in row["values"])
        default = f'Some("{rust_string(row["default"])}")' if row["default"] else "None"
        lines.append("    Keyword {")
        lines.append(f'        name: "{row["name"]}",')
        lines.append(f"        kind: Kind::{row['kind']},")
        lines.append(f"        values: &[{values}],")
        lines.append(f'        desc: "{rust_string(row["desc"])}",')
        lines.append(f'        aliases: "{rust_string(row["aliases"])}",')
        lines.append(f"        default: {default},")
        lines.append(f"        common: {str(row['common']).lower()},")
        lines.append(f"        repeatable: {str(row['repeatable']).lower()},")
        lines.append("    },")
    lines += [
        "];",
        "",
        "/// Looks a keyword up case-insensitively, as ssh itself does.",
        "pub fn lookup(name: &str) -> Option<&'static Keyword> {",
        "    KEYWORDS.iter().find(|k| k.name.eq_ignore_ascii_case(name))",
        "}",
        "",
        "/// The canonical spelling of `name`, or `name` unchanged if unknown.",
        "pub fn canonical(name: &str) -> String {",
        "    lookup(name).map_or_else(|| name.to_string(), |k| k.name.to_string())",
        "}",
        "",
    ]

    with open(os.path.join(os.path.dirname(__file__), "..", "src", "schema.rs"), "w") as f:
        f.write("\n".join(lines))
    print(f"wrote src/schema.rs: {len(rows)} keywords, "
          f"{sum(1 for r in rows if r['common'])} common, "
          f"{sum(1 for r in rows if r['default'])} with documented defaults")


if __name__ == "__main__":
    main()

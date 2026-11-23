/**
 * Answers a running `nocturne-server` gave, for the unit tests of this folder.
 *
 * Nothing here was written from the specification. A server was started on
 * an empty data directory; a two-switch lab was imported and one of its
 * switches configured from its console; net-snmp's captured trap datagrams
 * and hand-built NetFlow v5 / v9 datagrams were sent to the receivers over
 * UDP; forwarders and a rule were saved; and each method's answer was copied
 * here unchanged. `satisfies` makes the compiler check every one of them
 * against the type the screen is written to — a field the type lacks, or one
 * of the wrong kind, fails the build rather than a test.
 *
 * Ids, ports and timestamps are that run's. The trap community in that run
 * was a test value and appears nowhere below, because the engine returns it
 * nowhere.
 */

import type { LogEntry } from '@/api/types/core';
import type { Detection, DetectionHit } from '@/api/types/detect';
import type {
  DeviceSetup,
  FlowExporter,
  FlowTop,
  Forwarder,
  LogRule,
  LogsSettings,
  LogsStatus,
  RuleTest,
  TrapBody,
} from '@/api/types/logs';

/** `logs.status` after six trap datagrams and four flow datagrams, with one forwarder failing. */
export const STATUS = {
  "syslog": {
    "enabled": true,
    "listening": [
      "udp 127.0.0.1:54173"
    ],
    "error": null,
    "received": 0,
    "stored": 9,
    "dropped": 0,
    "queued": 0,
    "rate_per_minute": 0,
    "last_at": null,
    "from_lab": 9
  },
  "traps": {
    "enabled": true,
    "implemented": true,
    "listening": [
      "udp 127.0.0.1:38366"
    ],
    "error": null,
    "received": 6,
    "stored": 3,
    "dropped": 0,
    "queued": 0,
    "rate_per_minute": 6,
    "last_at": 1795404524368,
    "rejected": 2,
    "unauthenticated": 0,
    "malformed": 1,
    "duplicates": 0,
    "acknowledged": 0,
    "require_known_community": true,
    "rejections": [
      {
        "address": "127.0.0.2",
        "device_id": null,
        "reason": "no device in the inventory has the address 127.0.0.2, and no collected interface carries it",
        "count": 1,
        "last_at": 1795404524368
      },
      {
        "address": "127.0.0.1",
        "device_id": "kq4kwzyscmgnnvdt",
        "reason": "the community is not the one stored for `CORE1`",
        "count": 1,
        "last_at": 1795404524199
      }
    ],
    "lab": "A simulated device sends no traps, so nothing counted here came from a lab."
  },
  "flows": {
    "enabled": true,
    "implemented": true,
    "listening": [
      "udp 127.0.0.1:33323"
    ],
    "error": null,
    "received": 4,
    "malformed": 1,
    "records": 4,
    "counted": 4,
    "dropped": 0,
    "drops": {
      "queue_full": 0,
      "no_room": 0,
      "unclaimed": 0,
      "unreadable": 0
    },
    "queued": 0,
    "rate_per_minute": 3,
    "last_at": 1795404524769,
    "sets_without_template": 1,
    "exporters": 2,
    "unattributed_exporters": 1,
    "memory": {
      "minutes": 2,
      "keys": 22,
      "max_keys": 2000000,
      "evicted_minutes": 0,
      "oldest_at": 1795404480000
    },
    "lab": "A simulated device exports no flows, so nothing counted here came from a lab."
  },
  "rules": {
    "tracked": 12,
    "forgotten": 0,
    "max_per_rule": 10000,
    "held_back": 0,
    "open_alerts": 1,
    "undelivered": 0
  },
  "forwarders": [
    {
      "id": "vptgjx2zoyq22uoy",
      "name": "SIEM",
      "state": "failing",
      "sent": 0,
      "dropped": 2,
      "last_error": "connection refused by the destination",
      "last_at": null
    }
  ],
  "store": {
    "rows": 12,
    "oldest_at": 1795404461766,
    "retention_days": 30,
    "max_rows": 2000000,
    "last_prune_at": 1795404512089,
    "pruned_by_age": 0,
    "pruned_by_rows": 0
  },
  "unattributed_sources": 0
} satisfies LogsStatus;

/**
 * `logs.status` from a build whose queues are bounded in bytes as well as in
 * entries (`queued_bytes`, `max_queued_bytes`, and a forwarder's `queued`):
 * the same kind of run, a later engine.
 */
export const STATUS_WITH_QUEUE_BYTES = {
  "syslog": {
    "enabled": true,
    "listening": [
      "udp 127.0.0.1:56140"
    ],
    "error": null,
    "received": 4,
    "stored": 6,
    "dropped": 0,
    "queued": 0,
    "queued_bytes": 0,
    "max_queued_bytes": 67108864,
    "rate_per_minute": 4,
    "last_at": 1795420202835,
    "from_lab": 2
  },
  "traps": {
    "enabled": true,
    "implemented": true,
    "listening": [
      "udp 127.0.0.1:34394"
    ],
    "error": null,
    "received": 6,
    "stored": 3,
    "dropped": 0,
    "queued": 0,
    "queued_bytes": 0,
    "max_queued_bytes": 67108864,
    "rate_per_minute": 6,
    "last_at": 1795420203435,
    "rejected": 2,
    "unauthenticated": 0,
    "malformed": 1,
    "duplicates": 0,
    "acknowledged": 0,
    "require_known_community": true,
    "rejections": [
      {
        "address": "127.0.0.2",
        "device_id": null,
        "reason": "no device in the inventory has the address 127.0.0.2, and no collected interface carries it",
        "count": 1,
        "last_at": 1795420203435
      },
      {
        "address": "127.0.0.1",
        "device_id": "v7zgzoqnqhpsofy5",
        "reason": "the community is not the one stored for `CORE1`",
        "count": 1,
        "last_at": 1795420203135
      }
    ],
    "lab": "A simulated device sends no traps, so nothing counted here came from a lab."
  },
  "flows": {
    "enabled": true,
    "implemented": true,
    "listening": [
      "udp 127.0.0.1:59811"
    ],
    "error": null,
    "received": 4,
    "malformed": 1,
    "records": 4,
    "counted": 4,
    "dropped": 0,
    "drops": {
      "queue_full": 0,
      "no_room": 0,
      "unclaimed": 0,
      "unreadable": 0
    },
    "queued": 0,
    "queued_bytes": 0,
    "max_queued_bytes": 67108864,
    "rate_per_minute": 3,
    "last_at": 1795420204036,
    "sets_without_template": 1,
    "exporters": 2,
    "unattributed_exporters": 1,
    "memory": {
      "minutes": 2,
      "keys": 22,
      "max_keys": 2000000,
      "evicted_minutes": 0,
      "oldest_at": 1795420200000
    },
    "lab": "A simulated device exports no flows, so nothing counted here came from a lab."
  },
  "rules": {
    "tracked": 8,
    "forgotten": 0,
    "max_per_rule": 10000,
    "held_back": 0,
    "open_alerts": 1,
    "undelivered": 0
  },
  "forwarders": [
    {
      "id": "7tsjndbl6d54wmc2",
      "name": "Lake",
      "state": "disabled",
      "sent": 0,
      "dropped": 0,
      "queued": 0,
      "queued_bytes": 0,
      "max_queued_bytes": 16777216,
      "last_error": null,
      "last_at": null
    },
    {
      "id": "e7tfywl74t7ksari",
      "name": "SIEM",
      "state": "ok",
      "sent": 1,
      "dropped": 0,
      "queued": 0,
      "queued_bytes": 0,
      "max_queued_bytes": 16777216,
      "last_error": null,
      "last_at": 1795420219150
    }
  ],
  "store": {
    "rows": 9,
    "oldest_at": 1795420202553,
    "retention_days": 30,
    "max_rows": 2000000,
    "last_prune_at": null,
    "pruned_by_age": 0,
    "pruned_by_rows": 0
  },
  "unattributed_sources": 1
} satisfies LogsStatus;

/** `settings.get {section: 'logs'}` on a fresh server. */
export const SETTINGS = {
  "syslog": {
    "enabled": false,
    "udp_port": 5514,
    "tcp_port": 5514,
    "bind": "0.0.0.0"
  },
  "traps": {
    "enabled": false,
    "port": 1162,
    "bind": "0.0.0.0",
    "require_known_community": true
  },
  "flows": {
    "enabled": false,
    "port": 2055,
    "bind": "0.0.0.0"
  },
  "retention_days": 30,
  "max_rows": 2000000,
  "unknown_sources": "store",
  "queue_limit": 50000
} satisfies LogsSettings;

/** `logs.query {stream: 'trap'}`: an SNMPv1 linkDown, then a v2c linkUp and linkDown, newest first. */
export const TRAP_ENTRIES = [
  {
    "id": 37,
    "ts": 1795404524199,
    "stream": "trap",
    "device_id": "kq4kwzyscmgnnvdt",
    "severity": null,
    "source": "127.0.0.1",
    "message": "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)",
    "body": {
      "version": "v1",
      "trap": "linkDown",
      "oid": "1.3.6.1.6.3.1.1.5.3",
      "uptime": 123.45,
      "enterprise": "1.3.6.1.4.1.8072.3.2.10",
      "generic": 2,
      "specific": 0,
      "agent_addr": "192.0.2.1",
      "varbinds": [
        {
          "oid": "1.3.6.1.2.1.2.2.1.1.3",
          "name": "ifIndex.3",
          "value": "3"
        },
        {
          "oid": "1.3.6.1.2.1.2.2.1.7.3",
          "name": "ifAdminStatus.3",
          "value": "up(1)"
        },
        {
          "oid": "1.3.6.1.2.1.2.2.1.8.3",
          "name": "ifOperStatus.3",
          "value": "down(2)"
        }
      ],
      "if_index": 3
    }
  },
  {
    "id": 36,
    "ts": 1795404524199,
    "stream": "trap",
    "device_id": "kq4kwzyscmgnnvdt",
    "severity": null,
    "source": "127.0.0.1",
    "message": "linkUp ifIndex=3 ifAdminStatus=up(1) ifOperStatus=up(1)",
    "body": {
      "version": "v2c",
      "trap": "linkUp",
      "oid": "1.3.6.1.6.3.1.1.5.4",
      "uptime": 129.99,
      "varbinds": [
        {
          "oid": "1.3.6.1.2.1.2.2.1.1.3",
          "name": "ifIndex.3",
          "value": "3"
        },
        {
          "oid": "1.3.6.1.2.1.2.2.1.7.3",
          "name": "ifAdminStatus.3",
          "value": "up(1)"
        },
        {
          "oid": "1.3.6.1.2.1.2.2.1.8.3",
          "name": "ifOperStatus.3",
          "value": "up(1)"
        }
      ],
      "if_index": 3
    }
  },
  {
    "id": 33,
    "ts": 1795404524163,
    "stream": "trap",
    "device_id": "kq4kwzyscmgnnvdt",
    "severity": null,
    "source": "127.0.0.1",
    "message": "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)",
    "body": {
      "version": "v2c",
      "trap": "linkDown",
      "oid": "1.3.6.1.6.3.1.1.5.3",
      "uptime": 123.45,
      "varbinds": [
        {
          "oid": "1.3.6.1.2.1.2.2.1.1.3",
          "name": "ifIndex.3",
          "value": "3"
        },
        {
          "oid": "1.3.6.1.2.1.2.2.1.7.3",
          "name": "ifAdminStatus.3",
          "value": "up(1)"
        },
        {
          "oid": "1.3.6.1.2.1.2.2.1.8.3",
          "name": "ifOperStatus.3",
          "value": "down(2)"
        }
      ],
      "if_index": 3
    }
  }
] satisfies LogEntry[];

/** The body of the SNMPv1 trap above, checked against the type the drawer reads. */
export const V1_TRAP_BODY = {
  "version": "v1",
  "trap": "linkDown",
  "oid": "1.3.6.1.6.3.1.1.5.3",
  "uptime": 123.45,
  "enterprise": "1.3.6.1.4.1.8072.3.2.10",
  "generic": 2,
  "specific": 0,
  "agent_addr": "192.0.2.1",
  "varbinds": [
    {
      "oid": "1.3.6.1.2.1.2.2.1.1.3",
      "name": "ifIndex.3",
      "value": "3"
    },
    {
      "oid": "1.3.6.1.2.1.2.2.1.7.3",
      "name": "ifAdminStatus.3",
      "value": "up(1)"
    },
    {
      "oid": "1.3.6.1.2.1.2.2.1.8.3",
      "name": "ifOperStatus.3",
      "value": "down(2)"
    }
  ],
  "if_index": 3
} satisfies TrapBody;

/** A line a lab switch wrote and the lab handed over (`logs.ingest`): `via: 'lab'`, no source address. */
export const LAB_LINE = {
  "id": 23,
  "ts": 1795404462826,
  "stream": "syslog",
  "device_id": "fvkvbsh2tveiuow2",
  "severity": 5,
  "source": "",
  "message": "Configured from console by console",
  "body": {
    "raw": "*Mar  1 00:00:40.284: %SYS-5-CONFIG_I: Configured from console by console",
    "via": "lab",
    "severity_name": "notice",
    "timestamp": "*Mar  1 00:00:40.284",
    "mnemonic": "SYS-5-CONFIG_I"
  }
} satisfies LogEntry;

/** `logs.device_setup` for an IOS device, both receivers listening. */
export const SETUP_IOS = {
  "device_id": "kq4kwzyscmgnnvdt",
  "profile": "cisco_ios",
  "lines": [
    "service timestamps log datetime msec show-timezone",
    "logging origin-id hostname",
    "logging trap informational",
    "logging host 127.0.0.1 transport udp port 54173"
  ],
  "address": "127.0.0.1",
  "address_note": "the address on this machine that packets to 127.0.0.1 leave from",
  "udp_port": 54173,
  "note": "These lines are not applied: hand them to `changes.create` so they go through the change pipeline like any other configuration. The port is not the standard 514 because binding a port below 1024 needs privileges this application does not ask for.",
  "trap_lines": [
    "snmp-server enable traps snmp authentication linkdown linkup coldstart warmstart",
    "snmp-server host 127.0.0.1 version 2c <snmp-community> udp-port 38366"
  ],
  "trap_port": 38366,
  "trap_note": "These lines are not applied, and not complete. Put the SNMP community of this device where they say <snmp-community>: a community is a secret, and Nocturne does not put one into a line it returns. Then hand them to `changes.create`. The receiver keeps a notification from this device only when it carries exactly the community stored in its SNMP credential, so that is the one to use. SNMPv2c is unauthenticated UDP: whoever can reach the port can forge a notification, and its community travels in the clear. The port is not the standard 162 because binding a port below 1024 needs privileges this application does not ask for."
} satisfies DeviceSetup;

/** `flows.top {by: 'source', minutes: 15}`: two exporters, one sampling 1 in 100, one set without its template. */
export const TOP_BY_SOURCE = {
  "rows": [
    {
      "key": "10.0.0.5",
      "label": "10.0.0.5",
      "bytes": 49940,
      "packets": 46,
      "flows": 3
    },
    {
      "key": "10.0.0.7",
      "label": "10.0.0.7",
      "bytes": 9400,
      "packets": 50,
      "flows": 1
    }
  ],
  "total": {
    "bytes": 59340,
    "packets": 96,
    "flows": 4
  },
  "other": {
    "bytes": 0,
    "packets": 0,
    "flows": 0
  },
  "coverage": {
    "collecting": true,
    "window_start": 1795403580000,
    "until": 1795404526478,
    "since": 1795404480000,
    "records": 4,
    "sets_without_template": 1,
    "sampling": [
      100
    ],
    "sampled_records": 3,
    "exporters": 2
  }
} satisfies FlowTop;

/** `flows.top {by: 'interface'}`: the rows carry `interface` and add up to twice the total. */
export const TOP_BY_INTERFACE = {
  "rows": [
    {
      "key": "127.0.0.1 3 in",
      "label": "CORE1 ifIndex 3 in",
      "bytes": 34460,
      "packets": 74,
      "flows": 3,
      "interface": {
        "exporter": "127.0.0.1",
        "device_id": "kq4kwzyscmgnnvdt",
        "if_index": 3,
        "name": null,
        "direction": "in"
      }
    },
    {
      "key": "127.0.0.1 5 out",
      "label": "CORE1 ifIndex 5 out",
      "bytes": 34460,
      "packets": 74,
      "flows": 3,
      "interface": {
        "exporter": "127.0.0.1",
        "device_id": "kq4kwzyscmgnnvdt",
        "if_index": 5,
        "name": null,
        "direction": "out"
      }
    },
    {
      "key": "127.0.0.2 3 in",
      "label": "127.0.0.2 ifIndex 3 in",
      "bytes": 24880,
      "packets": 22,
      "flows": 1,
      "interface": {
        "exporter": "127.0.0.2",
        "device_id": null,
        "if_index": 3,
        "name": null,
        "direction": "in"
      }
    },
    {
      "key": "127.0.0.2 5 out",
      "label": "127.0.0.2 ifIndex 5 out",
      "bytes": 24880,
      "packets": 22,
      "flows": 1,
      "interface": {
        "exporter": "127.0.0.2",
        "device_id": null,
        "if_index": 5,
        "name": null,
        "direction": "out"
      }
    }
  ],
  "total": {
    "bytes": 59340,
    "packets": 96,
    "flows": 4
  },
  "other": {
    "bytes": 0,
    "packets": 0,
    "flows": 0
  },
  "coverage": {
    "collecting": true,
    "window_start": 1795403580000,
    "until": 1795404526482,
    "since": 1795404480000,
    "records": 4,
    "sets_without_template": 1,
    "sampling": [
      100
    ],
    "sampled_records": 3,
    "exporters": 2
  }
} satisfies FlowTop;

/** `flows.top` for one exporter that said no sampling rate. */
export const TOP_ONE_EXPORTER = {
  "rows": [
    {
      "key": "10.0.0.5",
      "label": "10.0.0.5",
      "bytes": 24880,
      "packets": 22,
      "flows": 1
    }
  ],
  "total": {
    "bytes": 24880,
    "packets": 22,
    "flows": 1
  },
  "other": {
    "bytes": 0,
    "packets": 0,
    "flows": 0
  },
  "coverage": {
    "collecting": true,
    "window_start": 1795403580000,
    "until": 1795404526484,
    "since": 1795404480000,
    "records": 1,
    "sets_without_template": 0,
    "sampling": [],
    "sampled_records": 0,
    "exporters": 1
  }
} satisfies FlowTop;

/** `flows.exporters`: one no device claims, one attributed. */
export const EXPORTERS = [
  {
    "address": "127.0.0.2",
    "device_id": null,
    "device": null,
    "reason": "no device in the inventory has the address 127.0.0.2, and no collected interface carries it",
    "version": 5,
    "first_at": 1795404524769,
    "last_at": 1795404524769,
    "datagrams": 1,
    "records": 1,
    "dropped": 0,
    "templates": 0,
    "sets_without_template": 0,
    "sampling": []
  },
  {
    "address": "127.0.0.1",
    "device_id": "kq4kwzyscmgnnvdt",
    "device": "CORE1",
    "reason": "",
    "version": 9,
    "first_at": 1795404524569,
    "last_at": 1795404524569,
    "datagrams": 2,
    "records": 3,
    "dropped": 0,
    "templates": 0,
    "sets_without_template": 1,
    "sampling": [
      100
    ]
  }
] satisfies FlowExporter[];

/** `logs.forward.list`: a syslog forwarder and an HTTPS one whose URL is in the vault. */
export const FORWARDERS = [
  {
    "id": "vptgjx2zoyq22uoy",
    "name": "SIEM",
    "enabled": true,
    "kind": "syslog_tcp",
    "host": "127.0.0.1",
    "port": 59849,
    "format": "rfc5424",
    "streams": [
      "syslog",
      "trap",
      "event"
    ],
    "min_severity": 6,
    "lab": true,
    "ca_pem": null,
    "has_secret": false,
    "updated_at": 1795404550616
  },
  {
    "id": "o2rslruwcxbpe7oh",
    "name": "Splunk (prod)",
    "enabled": false,
    "kind": "https_json",
    "host": "",
    "port": 0,
    "format": "json",
    "streams": [
      "syslog",
      "audit"
    ],
    "min_severity": null,
    "lab": false,
    "ca_pem": null,
    "has_secret": true,
    "updated_at": 1795404550622
  }
] satisfies Forwarder[];

/** Four of the built-in rules, as `logs.rules.list` returns them. */
export const RULE_LOGIN_BURST = {
  "id": "builtin.ios.login_burst",
  "name": "IOS: repeated failed logins from one address",
  "enabled": true,
  "builtin": true,
  "platform": "Cisco IOS / IOS-XE",
  "stream": "syslog",
  "pattern": "%SEC_LOGIN-4-LOGIN_FAILED: ?Login failed \\[user: ?(?P<user>.*)\\] \\[Source: ?(?P<source>[^\\]]*)\\] \\[localport: ?\\d+\\] \\[Reason:",
  "min_severity": null,
  "scope": {
    "device_ids": [],
    "sites": [],
    "roles": [],
    "tags": [],
    "profiles": []
  },
  "threshold": {
    "count": 8,
    "window_s": 60,
    "group_by": [
      "source"
    ]
  },
  "after": null,
  "clear_pattern": null,
  "clear_after_s": 600,
  "emit": {
    "kind": "alert",
    "severity": "high",
    "title": "Repeated failed logins from {source}"
  },
  "throttle_s": 60,
  "test_line": "%SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: 10.4.2.11] [localport: 22] [Reason: Login Authentication Failed] at 20:54:42 UTC Fri Feb 28 2003",
  "test_line_source": "Synthetic: written to the format `Login failed [user: [chars]] [Source: [chars]] [localport: [dec]] [Reason: [chars]] at [chars]` that Cisco's System Message Guide for IOS XE 16.12.x gives for SEC_LOGIN-4-LOGIN_FAILED (https://www.cisco.com/c/dam/en/us/td/docs/switches/lan/sys_messages/16-12/smg_1612x.xlsx); the values are filled in by hand, it is not a line a device sent",
  "updated_at": 1795404392077
} satisfies LogRule;
export const RULE_BGP = {
  "id": "builtin.ios.bgp_adjacency",
  "name": "IOS: BGP neighbour change",
  "enabled": true,
  "builtin": true,
  "platform": "Cisco IOS / IOS-XE",
  "stream": "syslog",
  "pattern": "%BGP-5-ADJCHANGE: neighbor (?P<neighbor>\\S+)(?: \\S+)*? Down\\b",
  "min_severity": null,
  "scope": {
    "device_ids": [],
    "sites": [],
    "roles": [],
    "tags": [],
    "profiles": []
  },
  "threshold": {
    "count": 1,
    "window_s": 60,
    "group_by": [
      "neighbor"
    ]
  },
  "after": null,
  "clear_pattern": "%BGP-5-ADJCHANGE: neighbor (?P<neighbor>\\S+)(?: \\S+)*? Up\\b",
  "clear_after_s": null,
  "emit": {
    "kind": "alert",
    "severity": "high",
    "title": "BGP neighbour {neighbor} is down"
  },
  "throttle_s": 60,
  "test_line": "*Mar  1 00:19:00.000: %BGP-5-ADJCHANGE: neighbor 192.0.2.1 Down BGP Notification sent",
  "test_line_source": "Cisco IOS / IOS-XE system message documentation",
  "updated_at": 1795404392077
} satisfies LogRule;
export const RULE_TRAP_LINK_DOWN = {
  "id": "builtin.trap.link_down",
  "name": "Trap: link down",
  "enabled": true,
  "builtin": true,
  "platform": "Any SNMP agent (SNMPv2-MIB, IF-MIB)",
  "stream": "trap",
  "pattern": "^linkDown\\b.*?\\bifIndex=(?P<ifindex>\\d+)(?P<name>(?: \\([^)]*\\))?)",
  "min_severity": null,
  "scope": {
    "device_ids": [],
    "sites": [],
    "roles": [],
    "tags": [],
    "profiles": []
  },
  "threshold": {
    "count": 1,
    "window_s": 60,
    "group_by": [
      "ifindex"
    ]
  },
  "after": null,
  "clear_pattern": "^linkUp\\b.*?\\bifIndex=(?P<ifindex>\\d+)",
  "clear_after_s": null,
  "emit": {
    "kind": "alert",
    "severity": "low",
    "title": "Link down: ifIndex {ifindex}{name}"
  },
  "throttle_s": 60,
  "test_line": "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)",
  "test_line_source": "Synthetic: the message this application's trap receiver writes for the SNMPv2c notification that net-snmp 5.9.4's `snmptrap` sends for `linkDown ifIndex.3 i 3 ifAdminStatus.3 i 1 ifOperStatus.3 i 2` (OIDs given as numbers). That datagram was captured and is replayed in this crate's tests/traps.rs, which checks that the message is this one. net-snmp sent it, not a device, and its values were chosen by hand",
  "updated_at": 1795404392077
} satisfies LogRule;
export const RULE_COLD_START = {
  "id": "builtin.trap.cold_start",
  "name": "Trap: device restarted (coldStart)",
  "enabled": true,
  "builtin": true,
  "platform": "Any SNMP agent (SNMPv2-MIB, IF-MIB)",
  "stream": "trap",
  "pattern": "^coldStart\\b",
  "min_severity": null,
  "scope": {
    "device_ids": [],
    "sites": [],
    "roles": [],
    "tags": [],
    "profiles": []
  },
  "threshold": null,
  "after": null,
  "clear_pattern": null,
  "clear_after_s": null,
  "emit": {
    "kind": "reload"
  },
  "throttle_s": 60,
  "test_line": "coldStart",
  "test_line_source": "Synthetic: the message this application's trap receiver writes for the SNMPv2c notification that net-snmp 5.9.4's `snmptrap` sends for `coldStart` (OIDs given as numbers). That datagram was captured and is replayed in this crate's tests/traps.rs, which checks that the message is this one. net-snmp sent it, not a device, and its values were chosen by hand",
  "updated_at": 1795404392077
} satisfies LogRule;

/** A rule of one's own, as `logs.rules.save` returned it. */
export const RULE_OWN = {
  "id": "rxl3b3dj3x5qgkjo",
  "name": "ACL denies",
  "enabled": true,
  "builtin": false,
  "platform": "",
  "stream": "syslog",
  "pattern": "%SEC-6-IPACCESSLOGP: list (?P<acl>\\S+) denied",
  "min_severity": null,
  "scope": {
    "device_ids": [],
    "sites": [],
    "roles": [],
    "tags": [],
    "profiles": []
  },
  "threshold": {
    "count": 3,
    "window_s": 60,
    "group_by": [
      "acl"
    ]
  },
  "after": null,
  "clear_pattern": null,
  "clear_after_s": 300,
  "emit": {
    "kind": "alert",
    "severity": "medium",
    "title": "{acl} is denying a lot"
  },
  "throttle_s": 60,
  "test_line": "",
  "test_line_source": "",
  "updated_at": 1795404552136
} satisfies LogRule;

/** `logs.rules.test`: a line that matches a rule with a threshold of three. */
export const TEST_THRESHOLD = {
  "matched": true,
  "reason": "",
  "groups": {
    "acl": "EDGE-IN"
  },
  "signal": {
    "kind": "alert",
    "title": "EDGE-IN is denying a lot",
    "severity": "medium",
    "interface": null
  },
  "parsed": {
    "severity": 6,
    "facility": 23,
    "mnemonic": "SEC-6-IPACCESSLOGP",
    "host": null,
    "message": "list EDGE-IN denied tcp 10.0.0.5(1234) -> 10.0.9.9(22), 1 packet"
  },
  "instance": "EDGE-IN",
  "note": "One such line raises nothing: the signal is raised when 3 of them arrive inside 60 s for the same device and the same acl. The count is kept in memory and starts again when Nocturne restarts.",
  "clears": false
} satisfies RuleTest;

/** `logs.rules.test` for the trap link rule: a linkDown message, its opposite, and a line that is neither. */
export const TEST_TRAP = {
  "matched": true,
  "reason": "",
  "groups": {
    "ifindex": "3",
    "name": " (GigabitEthernet0/3)"
  },
  "signal": {
    "kind": "alert",
    "title": "Link down: ifIndex 3 (GigabitEthernet0/3)",
    "severity": "low",
    "interface": null
  },
  "parsed": {
    "severity": null,
    "facility": null,
    "mnemonic": "linkDown",
    "host": null,
    "message": "linkDown ifIndex=3 (GigabitEthernet0/3) ifAdminStatus=up(1) ifOperStatus=down(2)"
  },
  "instance": "3",
  "note": "",
  "clears": false
} satisfies RuleTest;
export const TEST_TRAP_CLEARS = {
  "matched": false,
  "reason": "this line is the rule's opposite event: it raises nothing and clears the alert the rule has open for this instance",
  "groups": {
    "ifindex": "3"
  },
  "signal": null,
  "parsed": {
    "severity": null,
    "facility": null,
    "mnemonic": "linkUp",
    "host": null,
    "message": "linkUp ifIndex=3 ifAdminStatus=up(1) ifOperStatus=up(1)"
  },
  "instance": "3",
  "note": "",
  "clears": true
} satisfies RuleTest;
export const TEST_TRAP_NO_MATCH = {
  "matched": false,
  "reason": "the pattern does not match this line",
  "groups": {},
  "signal": null,
  "parsed": {
    "severity": null,
    "facility": null,
    "mnemonic": "coldStart",
    "host": null,
    "message": "coldStart"
  },
  "instance": "",
  "note": "",
  "clears": false
} satisfies RuleTest;

/** `detections.list` on a fresh server: seven, in catalogue order. */
export const CATALOGUE = [
  {
    "id": "unplanned_config_change",
    "name": "Unplanned configuration change",
    "description": "A device's configuration changed while no approved change was being applied to it.",
    "severity": "high",
    "decided_from": "A log line in which the device says it was configured, with who and from where when it says so (`logs.config_changed`), and a backup that differs from the one before it (`config.changed`, with when each version was seen from `config.versions`) — each compared with the changes the pipeline ran on that device (`changes.list`).",
    "cannot_see": "A change on a device that sends no syslog and is not backed up before the next schedule. Who made it, when the device does not log that. A change by hand made between the same two backups as an approved one — including one the pipeline's own backup, taken as an approved change starts, is the first to see.",
    "enabled": true,
    "available": true,
    "hits_24h": 0
  },
  {
    "id": "unplanned_reload",
    "name": "Unplanned restart",
    "description": "A device restarted and nothing Nocturne was doing explains it.",
    "severity": "medium",
    "decided_from": "A log rule's `reload` signal (`logs.signal`), compared with the changes the pipeline ran on that device (`changes.list`), with jobs that can change a device (`changes.apply`, `changes.rollback`, `automation.ansible`) and, for a simulated device, with its lab being started, reset or given a new startup configuration (the audit trail).",
    "cannot_see": "A restart of a device that sends no syslog. Why it restarted: power, a crash and somebody typing `reload` at its console all look the same from here.",
    "enabled": true,
    "available": true,
    "hits_24h": 0
  },
  {
    "id": "host_key_changed",
    "name": "SSH host key changed",
    "description": "A device presented an SSH host key that is not the one Nocturne remembers for it.",
    "severity": "critical",
    "decided_from": "`connect.host_key_changed`, published by the core whenever a connection meets a key that differs from the remembered one.",
    "cannot_see": "A key that changed on a device nobody has connected to since. Whether the device was replaced or re-keyed, or the connection is being intercepted: both look exactly like this.",
    "enabled": true,
    "available": true,
    "hits_24h": 0
  },
  {
    "id": "unredacted_config_views",
    "name": "Repeated unredacted configuration views",
    "description": "Configurations were read with their secrets showing, several times in a short while.",
    "severity": "medium",
    "decided_from": "The audit trail: `config.view_unredacted` entries, counted over a window.",
    "cannot_see": "Who: Nocturne has one operator identity, not users. An unredacted export to disk, which is one audit entry (`config.export`) however many devices it covers.",
    "enabled": true,
    "available": true,
    "hits_24h": 0
  },
  {
    "id": "secrets_changed",
    "name": "Credentials or master password changed",
    "description": "The secrets of a stored credential were changed, a credential was deleted, or the vault's master password was set, changed or removed. A new credential is not a hit.",
    "severity": "info",
    "decided_from": "The audit trail: `credential.save` entries that say an existing credential's secrets were added, replaced or removed, `credential.delete` and `vault.set_password` entries.",
    "cannot_see": "A save recorded before the audit entry said what it changed. Who did it.",
    "enabled": true,
    "available": true,
    "hits_24h": 0
  },
  {
    "id": "failed_unlocks",
    "name": "Repeated failed unlocks",
    "description": "Wrong vault passwords, or requests to the server with a wrong access token, several times in a short while.",
    "severity": "high",
    "decided_from": "The audit trail: `vault.wrong_password` entries, and `server.token_refused` entries, which fold a minute of refused requests into one entry with a count.",
    "cannot_see": "Somebody who guesses right the first time. Requests with no token at all are recorded but not counted here: that is a scanner or a stale browser tab, not a guess.",
    "enabled": true,
    "available": true,
    "hits_24h": 0
  },
  {
    "id": "protected_change_outside_window",
    "name": "Protected change outside a maintenance window",
    "description": "An approved change touched protected resources on a device that no maintenance window covered at the time.",
    "severity": "info",
    "decided_from": "`changes.changed`, when a change with protected hits finishes, compared with the maintenance windows (`maintenance.list`).",
    "cannot_see": "Whether the change was right: it was approved, by a person, with the protected resources named. This only says nobody had announced the work. A device whose change failed: the summary does not say whether anything was sent to it before it failed. When each device of a long change was done: the change is judged at the moment all of it finished.",
    "enabled": true,
    "available": true,
    "hits_24h": 0
  }
] satisfies Detection[];

/** `detections.recent`: a lab switch configured from its console with no change in the pipeline. */
export const HIT = {
  "id": "slgxpq4v6kdlmyiw",
  "detection": "unplanned_config_change",
  "at": 1795404462826,
  "device_id": "fvkvbsh2tveiuow2",
  "severity": "high",
  "title": "console changed LogLab/SW1 from console and no approved change was being applied",
  "evidence": {
    "decided_from": "log",
    "log_entry_id": 23,
    "stream": "syslog",
    "via": "lab",
    "by": "console",
    "from": "console",
    "changes_checked": 0,
    "grace_minutes": 15
  },
  "alert_id": "6riqtt4myyynjkj3",
  "lab": true
} satisfies DetectionHit;

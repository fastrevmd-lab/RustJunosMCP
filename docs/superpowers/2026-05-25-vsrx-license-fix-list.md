# vSRX License & Reachability Fix List

**Date:** 2026-05-25
**Source:** Live `show system license` audit run from LXC 601 (rust-junosmcp) against all 23 MCP-registered devices.
**Goal:** Identify devices that need a full IDP-SIG + APPID Signature license (or are unreachable) before kicking off Phase 2 (srxmcp v0.2.0 — IDP / AppID signature-package lifecycle).

---

## Summary

| Status | Count |
|---|---|
| Licensed (IDP-SIG + APPID, ≥1 year) | 9 |
| Trial-only — needs full license | 10 |
| Offline / unreachable | 4 |
| **Total in inventory** | **23** |

Phase 2 work needs **only one** licensed device to capture fixtures; `vSRX-CI-tester` is the chosen lab box. The list below is for fleet hygiene + future expansion.

---

## Needs full license (10 devices, trial-only)

These devices have only the two evaluation trials (`E20210617001` Virtual Appliance + `E20210617002` VCPU Scale). They have **no IDP-SIG, APPID, AV, or Web-Filtering** licenses, and their trials expire over the next ~7 weeks.

| MCP name | IP | Trial expiry | Note |
|---|---|---|---|
| vSRX-test6 | 192.168.1.228 | 2026-07-21 | |
| vSRX-test7 | 192.168.1.229 | 2026-07-03 | |
| vSRX-test8 | 192.168.1.230 | 2026-07-15 | |
| vSRX-test9 | 192.168.1.231 | 2026-07-15 | |
| vSRX-test10 | 192.168.1.232 | 2026-07-02 | Used in lab IPsec tunnel test10↔test11 |
| vSRX-test11 | 192.168.1.233 | 2026-07-02 | Used in lab IPsec tunnel test10↔test11 |
| vSRX-test12 | 192.168.1.234 | 2026-07-09 | |
| vSRX-test16 | 192.168.1.238 | 2026-06-29 | Earliest expiry |
| vSRX-test17 | 192.168.1.239 | 2026-06-29 | Earliest expiry |
| vSRX-test18 | 192.168.1.240 | 2026-06-29 | Earliest expiry |

**Recommended action:** Request demolab or commercial bundle covering IDP-SIG, APPID Signature, Web Filtering, AV, ATP Cloud, and Virtual Appliance + VCPU Scale (a "DemolabJUNOSxxxxxxxxx"-style 1-year package matches what the already-licensed `vSRX-test1/test3/test4/CI-tester` boxes carry).

---

## Offline / unreachable (4 devices)

These responded with SSH connection failures during the audit. Either powered off, on a different VLAN, or NETCONF not enabled.

| MCP name | IP | Failure |
|---|---|---|
| mnha-router | 192.168.1.235 | `No route to host` (host unreachable) |
| vSRX-Node1 | 192.168.1.236 | `No route to host` |
| vSRX-Node2 | 192.168.1.237 | `operation timed out after 360s` |
| vSRX-Production | 192.168.1.222 | `Connection reset by peer` |

**Recommended action:** Verify each VM is powered on, on the management VLAN, and has `set system services netconf ssh` configured. Re-audit after fixes.

---

## Already licensed — no action needed (9 devices)

For reference. These have IDP-SIG + APPID Signature for at least one year out.

| MCP name | IP | License source | IDP/APPID expiry |
|---|---|---|---|
| vSRX-CI-tester | 192.168.1.227 | Demolab demo | 2027-05-22 |
| vSRX-mm-A | 192.168.1.242 | Commercial `JUNOS145915511` | 2028-12-12 |
| vSRX-mm-B | 192.168.1.243 | Commercial `JUNOS145915511` | 2028-12-12 |
| vSRX-test1 | 192.168.1.244 | Demolab demo | 2027-05-22 |
| vSRX-test2 | 192.168.1.224 | Commercial `DEMOLABJUNOS505943029` | 2027-03-13 |
| vSRX-test3 | 192.168.1.220 | Demolab demo | 2027-05-22 |
| vSRX-test4 | 192.168.1.226 | Demolab demo | 2027-05-22 |
| vSRX-test19-20 | 192.168.1.241 | Demolab demo | 2027-05-22 (special: 16 VCPU) |
| vSRX-twin | 192.168.1.223 | Demolab demo | 2027-05-21 |

---

## Side-note for `check_srx_feature_license` parser (srxmcp)

The audit surfaced a **license-name format split** between older and newer Junos demolab packages:

- **Older style** (test1/test3/test4/CI-tester, expiring 2027-05-22): `IDP-SIG`, `APPID Signature`, `Anti-Spam`, `Sophos AV`, `Web Filtering EWF`
- **Newer style** (mm-A/mm-B): `idp-sig`, `appid-sig`, `av_key_sophos_engine`, `wf_key_websense_ewf`, `wf_key_ng_juniper`, `anti_spam_key_sbl`

`check_srx_feature_license` currently uses TitleCase tokens — the lowercase variant will read as `not_configured` on mm-A/mm-B even though the licenses are present. Track as a separate srxmcp issue (parser hardening) — not blocking Phase 2 fixture work.

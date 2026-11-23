# Two editions from one tree

Nocturne serves two people who want different things:

* an **IT engineer or homelab owner** who wants the daily work of a network
  done well, for nothing;
* an **ISP** who wants that *and* to know who their subscribers are, what
  each was sold, what each owes and whether what they use matches what they
  pay for — and who will pay for it.

This is how they are kept apart.

## Not two standalones

The obvious answer is two products. It is the wrong one here, for a specific
reason rather than a stylistic one: **the ISP value comes from the NOC
features.** "This customer pays for 10 Mbit and is pulling 14", "this tower is
oversubscribed three to one", "their link degraded two days before they
phoned" — every one of those is interface rates, monitoring and collected
device data that the free edition already gathers. Two standalones means
maintaining the device drivers, the pollers and the time series twice, and the
billing product would get the worse copy.

## One tree, two builds

The separation needs no new mechanism, because the architecture already has
one. A feature is a crate that registers itself on an `AppBuilder`; features
reach each other **by RPC method name and cope when a method is absent**; and
every screen already asks `app.methods` and says so in one sentence when a
feature is not in the build.

So:

| | Free edition | ISP edition |
|---|---|---|
| Ships as | the desktop application and a server binary | the same two, built separately |
| Features | the fourteen NOC crates | the same **plus `nocturne-isp`** |
| How | `nocturne-isp` is not compiled in | it is |

Both editions ship as both shapes. An earlier draft of this document said
the ISP edition was the server and the free one the desktop application,
which was a guess about who wants what: a small provider running one office
wants a window on their own machine as much as anyone, and a homelab with a
rack wants a server. The edition decides what is *in* the build; the shape
is the user's to choose.

The ISP desktop build is a **separate product** — "Nocturne ISP", its own
identifier, its own installer filenames — so the two can be installed side
by side and neither replaces the other. That matters for the obvious case:
trying the paid edition without losing the free one.

`nocturne-suite` carries an optional `isp` feature; `nocturne-server` and
`nocturne-desktop` pass it through. The default build does not depend on
`nocturne-isp` at all, so:

* the free binary does not *contain* the billing code — not disabled, not
  dead, absent;
* the ISP screens disappear on their own, through the gate every screen
  already has;
* and the suite's own test asserts the free edition registers no `isp.*`
  method, so the separation is **verified rather than trusted**.

## How an edition is told apart at runtime: it is not

There is no edition flag, no licence boolean, no "mode" setting. A client-side
switch that unlocks paid features is security theatre: anyone who can run the
binary can flip it, and honest users carry the cost of the checks.

The commercial control is **distribution**, and nothing else. The ISP build
is a different artefact, and what a customer buys is access to it. If it is
hosted, the entitlement is simply the account on the hosted service and there
is nothing on the client to defeat.

No licence file is read or checked anywhere, by the server or the desktop
application of either edition: an ISP build starts and serves its billing
methods exactly as the free build serves its own. A signed licence for a
self-hosted ISP server has been discussed, as a receipt that names the
licensee rather than a lock, and would add nothing to the control above. It
is not built, and this document will say so here until it is.

What the release does verify is that each artefact is the edition it is
named for (`.github/workflows/release.yml`): every server binary is started
and asked for its methods, and every desktop installer is unpacked and its
program asked what it is (`--version`, which says `(ISP edition)` when the
assembled engine registers the subscriber ledger's methods). An installer or
binary that is not its edition fails the job and is never uploaded.

This is deliberately a weak technical control and a strong commercial one. The
alternative — obfuscation, phoning home, time bombs — costs more to build,
annoys the people who paid, and stops nobody who is determined.

## What the free edition must never become

A demo. It is not crippled, it is not time-limited, and it does not nag. It is
the whole network-operations tool: fourteen features, every screen, the change
pipeline, the simulator and the assistant. What it does not have is
somebody else's subscribers and somebody else's money, which an IT engineer
has no use for anyway.

That is the test for whether a future feature belongs behind the edition
line: **does an IT engineer at one organisation want it?** Monitoring, backups
and safe changes — yes, free. Invoicing a third party, arrears ageing, margin
per megabit — no, those exist because somebody resells bandwidth.

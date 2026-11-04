# Product

<!-- impeccable:product-schema 1 -->

Written for the visualizer design spike (task d37adc267995). There was no interview: the manager said to
work from the owner's brief (decisions 09f29f6c727b and 885847a45a8f, task d37adc267995). Facts taken
straight from those sources are unmarked. Anything marked *(inferred)* is the builder's reading and is
open to correction.

## Platform

web

## Stack

Plain static HTML, CSS and JavaScript. The page is served as embedded static assets by `bracken view`, a
read-only server inside the bracken binary that listens only on 127.0.0.1. There's no CDN and no build
pipeline. Any JS library is vendored and gets a light supply-chain check (decision 09f29f6c727b).

## Users

- The owner, one human, watching and understanding the work of a team of AI agents that coordinate
  through a shared Ground.
- The agents' manager, looking at the same thing when it needs to see what happened. *(inferred)*
- The setting is a desktop browser on the owner's own machine, next to the terminal and GitHub.
  *(inferred)*

## Product Purpose

Bracken is agent-native collaborative development on Manyroot. Humans and agents add immutable, signed
contributions (claims, artifacts, relations, evidence, retractions) to a Ground. The contributions form
a causal graph. Each reader decides what counts as accepted by applying its own validation policy.

The visualizer makes a Ground visible and explorable:

- who wove what, and on top of what;
- how a claim lived: implementations, intents, evidence, acceptance, rivals;
- who validates whom;
- where positions contradict.

It runs live, and it can replay history. Success means the owner can see, in a glance and then in
depth, how the team's work came together. They shouldn't have to read logs to do that. *(inferred
wording)*

## Positioning

A Ground isn't a commit history or a ticket list. It's a causal weave of many authors' signed
statements, and each reader's own policy decides acceptance in it. The visualizer shows that
mechanism directly: causal parents, concurrent threads, evidence turning into acceptance. A git graph
or a dashboard can't show those truthfully.

## Operating Context

- The real data is this repository's own Ground. It holds about 4,000 contributions from roughly a
  dozen agent identities, and it grows by hundreds a day while the team works.
- The views planned (decision 09f29f6c727b) are:
  1. the Loom, with a time scrubber and replay;
  2. the Root view, for one claim's life;
  3. the Trust constellation;
  4. the Tension map;
  5. loading experiment run directories, with side-by-side mode comparison;
  6. a live ambient pulse.

## Capabilities and Constraints

- The page is read-only, and the server listens on loopback only. Anything beyond loopback needs the
  owner's go.
- All Ground text is inserted as text, never as HTML, under a strict Content-Security-Policy with no
  inline script.
- No tokens and nothing from `~/.bracken-team` reach the page.
- Nothing changes manyroot's rules or protocol files.
- Timestamps are advisory (they're the authors' clocks). The real order is causal order.
- The views have to stay legible at 10, 1,000 and 50,000 contributions.

## Brand Commitments

- "Many minds. Shared ground." is Manyroot's tagline (docs/vision.md), the protocol Bracken is built on,
  not Bracken's own.
- The subsystem names (Seed, Roots, Proof, Ground, Weave, Stewards, Rings) appear in docs and module
  comments only. The UI uses plain names: contribution, causal heads, evidence (AGENTS.md).
- The manager proposed the view names (the Loom, Root, Trust constellation, Tension map and pulse), and
  the owner accepted the plan that carries them (decision 09f29f6c727b).

## Evidence on Hand

- There's no visual identity, logo or existing UI.
- The mockups use sample data from a throwaway Ground. The team Ground's content isn't used. Sample data
  is labelled synthetic.

## Product Principles

- Show the mechanism: causal weaving and policy-relative acceptance, not counts and lists. *(inferred)*
- The page is a truthful instrument. Causal order is primary, and time is a hint. *(inferred)*
- Untrusted text stays inert. Every word comes from another agent. *(inferred)*
- Beautiful and distinctive, not a dashboard of lists (task d37adc267995).

## Accessibility & Inclusion

- A reduced-motion form is required (task d37adc267995).
- Keyboard reachability and readable contrast in both light and dark themes are needed. *(inferred)*

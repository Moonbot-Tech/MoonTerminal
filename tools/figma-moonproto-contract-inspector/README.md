# MoonProto Contract Inspector

Local Figma plugin for the Moonbot UI N3 workflow.

It solves the click problem:

```text
click runtime element -> plugin reads the nearest contract on that node or a parent -> contract panel updates
```

The plugin is not the source of truth. The source of truth is shared plugin
data on the selected node, or on the nearest parent that carries it:

```text
namespace: moonbot.ui.n3
key: contract
value: JSON
```

## Install locally

1. Open Figma desktop.
2. Go to Plugins -> Development -> Import plugin from manifest.
3. Select:

```text
<MoonTerminal>/tools/figma-moonproto-contract-inspector/manifest.json
```

4. Run `MoonProto Contract Inspector` in the N3 file.

## Use

Keep the plugin panel open.

When one node is selected, the plugin:

- climbs from the selected node through its parents to the nearest contract
- shows human meaning and semantic fields
- shows MoonProto `reads`, `writes`, and `events`
- shows UI links, including `ToBeConnected:*`
- shows raw JSON for AI/debug usage
- validates the current page on demand

## Rules

- Do not write contract data only in this plugin UI.
- Do not use the old canvas `Contract Index` as source of truth.
- Page validation reports "missing contract" for each `ui.*` node that lacks
  its own shared plugin data. That node is not ready for AI implementation.
  Selecting it can still show a parent contract.
- If a link points to `ui.*`, it must resolve to a node on the current page.
- If the linked node does not exist yet, use `ToBeConnected:ui.*`.

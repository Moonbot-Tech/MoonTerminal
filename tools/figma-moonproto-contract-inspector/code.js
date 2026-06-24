const NAMESPACE = "moonbot.ui.n3";
const CONTRACT_KEY = "contract";
const UI_LINK_FIELDS = [
  "opens",
  "targets",
  "updates_state",
  "owns",
  "owned_by",
  "depends_on",
];

figma.showUI(__html__, {
  width: 460,
  height: 680,
  title: "MoonProto Contract",
});

function stableIdOf(node) {
  if (!node || !node.name) return "";
  return node.name.split("|")[0].trim();
}

function readContract(node) {
  if (!node || typeof node.getSharedPluginData !== "function") return null;
  const raw = node.getSharedPluginData(NAMESPACE, CONTRACT_KEY);
  if (!raw) return null;
  try {
    return { raw, json: JSON.parse(raw), parseError: null };
  } catch (error) {
    return { raw, json: null, parseError: String(error && error.message ? error.message : error) };
  }
}

function findContractOwner(node) {
  let current = node;
  while (current) {
    const contract = readContract(current);
    if (contract) return { owner: current, contract };
    current = current.parent;
  }
  return null;
}

function collectNodeTrail(node) {
  const trail = [];
  let current = node;
  while (current && current.type !== "DOCUMENT") {
    trail.push({
      id: current.id,
      name: current.name || "",
      type: current.type,
      stableId: stableIdOf(current),
    });
    current = current.parent;
  }
  return trail;
}

function summarizeSelection() {
  const selection = figma.currentPage.selection;
  if (!selection.length) {
    return {
      state: "empty",
      namespace: NAMESPACE,
      key: CONTRACT_KEY,
      page: figma.currentPage.name,
    };
  }

  if (selection.length !== 1) {
    return {
      state: "multiple",
      namespace: NAMESPACE,
      key: CONTRACT_KEY,
      page: figma.currentPage.name,
      count: selection.length,
      selected: selection.map((node) => ({
        id: node.id,
        name: node.name,
        type: node.type,
        stableId: stableIdOf(node),
      })),
    };
  }

  const selected = selection[0];
  const match = findContractOwner(selected);
  if (!match) {
    return {
      state: "missing",
      namespace: NAMESPACE,
      key: CONTRACT_KEY,
      page: figma.currentPage.name,
      selected: {
        id: selected.id,
        name: selected.name,
        type: selected.type,
        stableId: stableIdOf(selected),
      },
      trail: collectNodeTrail(selected),
    };
  }

  const { owner, contract } = match;
  return {
    state: contract.parseError ? "invalid_json" : "ready",
    namespace: NAMESPACE,
    key: CONTRACT_KEY,
    page: figma.currentPage.name,
    selected: {
      id: selected.id,
      name: selected.name,
      type: selected.type,
      stableId: stableIdOf(selected),
    },
    owner: {
      id: owner.id,
      name: owner.name,
      type: owner.type,
      stableId: stableIdOf(owner),
      isSelected: owner.id === selected.id,
    },
    contract: contract.json,
    raw: contract.raw,
    parseError: contract.parseError,
    trail: collectNodeTrail(selected),
  };
}

function postSelection() {
  figma.ui.postMessage({
    type: "selection",
    payload: summarizeSelection(),
  });
}

function isUiStableId(id) {
  return typeof id === "string" && id.startsWith("ui.");
}

function isToBeConnected(id) {
  return typeof id === "string" && id.startsWith("ToBeConnected:ui.");
}

function findNodeByStableId(stableId) {
  return figma.currentPage.findOne((node) => {
    if (!node || !node.name) return false;
    return stableIdOf(node) === stableId;
  });
}

function collectContractLinks(contract) {
  const out = [];
  for (const field of UI_LINK_FIELDS) {
    const value = contract[field];
    if (Array.isArray(value)) {
      for (const item of value) {
        if (typeof item === "string") out.push({ field, target: item });
        if (item && typeof item === "object") {
          for (const key of ["target", "state", "id"]) {
            if (typeof item[key] === "string") out.push({ field: `${field}.${key}`, target: item[key] });
          }
        }
      }
    }
  }
  if (Array.isArray(contract.actions)) {
    for (const item of contract.actions) {
      if (item && typeof item === "object") {
        if (typeof item.target === "string") out.push({ field: "actions.target", target: item.target });
        if (typeof item.state === "string") out.push({ field: "actions.state", target: item.state });
      }
    }
  }
  return out;
}

function validateContract(node, contract, ids) {
  const errors = [];
  const nameId = stableIdOf(node);
  if (!contract || typeof contract !== "object") {
    errors.push("contract is not an object");
    return errors;
  }
  if (contract.id !== nameId) errors.push(`contract.id '${contract.id}' != layer stable id '${nameId}'`);
  if (contract.kind !== "MoonProtoBound" && contract.kind !== "TerminalOnly") errors.push(`bad kind '${contract.kind}'`);
  if (!contract.semantic || typeof contract.semantic !== "object") {
    errors.push("missing semantic object");
  } else {
    if (!contract.semantic.human_name) errors.push("missing semantic.human_name");
    if (!contract.semantic.concept) errors.push("missing semantic.concept");
    if (!contract.semantic.meaning) errors.push("missing semantic.meaning");
  }
  if (contract.kind === "MoonProtoBound") {
    const hasApi =
      Array.isArray(contract.reads) ||
      Array.isArray(contract.writes) ||
      Array.isArray(contract.events) ||
      (Array.isArray(contract.missing_api) && contract.missing_api.length > 0);
    if (!hasApi) errors.push("MoonProtoBound without reads/writes/events/missing_api");
  }
  if (contract.kind === "TerminalOnly") {
    const hasPurpose = Boolean(contract.purpose || (contract.semantic && contract.semantic.meaning));
    const hasActions = Array.isArray(contract.actions) && contract.actions.length > 0;
    if (!hasPurpose && !hasActions) errors.push("TerminalOnly without purpose/actions");
  }
  for (const link of collectContractLinks(contract)) {
    if (isToBeConnected(link.target)) continue;
    if (!isUiStableId(link.target)) {
      errors.push(`${link.field} '${link.target}' is not ui.* or ToBeConnected:ui.*`);
      continue;
    }
    if (!ids.has(link.target)) errors.push(`${link.field} target '${link.target}' not found on current page`);
  }
  return errors;
}

function validateCurrentPage() {
  const nodes = figma.currentPage.findAll((node) => node.name && stableIdOf(node).startsWith("ui."));
  const ids = new Map();
  for (const node of nodes) {
    const id = stableIdOf(node);
    if (!ids.has(id)) ids.set(id, []);
    ids.get(id).push(node.id);
  }

  const errors = [];
  let moonProtoBound = 0;
  let terminalOnly = 0;
  let toBeConnected = 0;
  let missingApi = 0;

  for (const node of nodes) {
    const id = stableIdOf(node);
    const duplicate = ids.get(id);
    if (duplicate && duplicate.length > 1) {
      errors.push({ id, nodeId: node.id, error: `duplicate stable id: ${duplicate.join(", ")}` });
    }

    const contract = readContract(node);
    if (!contract) {
      errors.push({ id, nodeId: node.id, error: "missing contract" });
      continue;
    }
    if (contract.parseError) {
      errors.push({ id, nodeId: node.id, error: `invalid JSON: ${contract.parseError}` });
      continue;
    }

    if (contract.json.kind === "MoonProtoBound") moonProtoBound++;
    if (contract.json.kind === "TerminalOnly") terminalOnly++;
    const raw = contract.raw;
    const tbcMatches = raw.match(/ToBeConnected:ui\\./g);
    if (tbcMatches) toBeConnected += tbcMatches.length;
    if (Array.isArray(contract.json.missing_api)) missingApi += contract.json.missing_api.length;

    for (const error of validateContract(node, contract.json, ids)) {
      errors.push({ id, nodeId: node.id, error });
    }
  }

  return {
    page: figma.currentPage.name,
    valid: errors.length === 0,
    uiNodeCount: nodes.length,
    moonProtoBound,
    terminalOnly,
    toBeConnected,
    missingApi,
    errors,
  };
}

figma.on("selectionchange", postSelection);
figma.on("currentpagechange", postSelection);

figma.ui.onmessage = async (message) => {
  if (!message || typeof message !== "object") return;

  if (message.type === "refresh") {
    postSelection();
    return;
  }

  if (message.type === "validate") {
    figma.ui.postMessage({ type: "validation", payload: validateCurrentPage() });
    return;
  }

  if (message.type === "selectOwner" && typeof message.nodeId === "string") {
    const node = await figma.getNodeByIdAsync(message.nodeId);
    let resolved = false;
    let error = null;
    if (node && node.type !== "DOCUMENT" && node.type !== "PAGE") {
      try {
        figma.currentPage.selection = [node];
        figma.viewport.scrollAndZoomIntoView([node]);
        resolved = true;
      } catch (err) {
        error = String(err && err.message ? err.message : err);
      }
    }
    figma.ui.postMessage({
      type: "selectStableIdResult",
      payload: { requested: message.nodeId, resolved, nodeId: resolved ? node.id : null, error },
    });
    postSelection();
    return;
  }

  if (message.type === "selectStableId" && typeof message.stableId === "string") {
    const id = message.stableId.startsWith("ToBeConnected:") ? message.stableId.slice("ToBeConnected:".length) : message.stableId;
    const node = findNodeByStableId(id);
    let resolved = false;
    let error = null;
    if (node) {
      try {
        figma.currentPage.selection = [node];
        figma.viewport.scrollAndZoomIntoView([node]);
        resolved = true;
      } catch (err) {
        error = String(err && err.message ? err.message : err);
      }
    }
    figma.ui.postMessage({
      type: "selectStableIdResult",
      payload: { requested: message.stableId, resolved, nodeId: resolved && node ? node.id : null, error },
    });
    postSelection();
  }
};

postSelection();

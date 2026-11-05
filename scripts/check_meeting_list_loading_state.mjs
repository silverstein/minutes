#!/usr/bin/env node

import { readFileSync } from "node:fs";

const PANEL = "tauri/src/index.html";
const panel = readFileSync(PANEL, "utf8");

function requireContract(condition, message) {
  if (!condition) {
    console.error(`meeting list loading state: ${message}`);
    process.exit(1);
  }
}

const listStart = panel.indexOf('<div class="meetings-scroll" id="meetings-scroll"');
const listEnd = panel.indexOf("<!-- Footer -->", listStart);
requireContract(listStart !== -1 && listEnd !== -1, "could not locate initial meeting list markup");

const initialList = panel.slice(listStart, listEnd);
requireContract(
  initialList.includes('id="meeting-list-loading"') && initialList.includes("Loading meetings…"),
  "initial meeting list must render the loading state",
);
requireContract(
  (initialList.match(/data-meeting-skeleton/g) || []).length === 3,
  "initial loading state must contain exactly three skeleton meeting cards",
);
requireContract(
  !initialList.includes("No meetings yet"),
  'initial meeting list must not claim "No meetings yet"',
);

const loadStart = panel.indexOf("async function loadMeetings(options = {})");
const loadingStart = panel.indexOf("beginMeetingInventoryLoading();", loadStart);
// loadMeetings reaches cmd_list_meetings through requestMeetingInventory (or
// its patient wrapper), so check the call order there and that the helper
// still issues the request.
const inventoryStart = Math.min(
  ...["requestMeetingInventory(", "invokeInventoryPatiently("]
    .map((call) => panel.indexOf(call, loadStart))
    .filter((index) => index !== -1),
);
const helperStart = panel.indexOf("function requestMeetingInventory(");
requireContract(
  loadStart !== -1 && loadingStart > loadStart && inventoryStart > loadingStart,
  "loadMeetings must enter loading before starting the inventory request",
);
requireContract(
  helperStart !== -1 &&
    panel.indexOf("invoke('cmd_list_meetings'", helperStart) > helperStart &&
    panel.indexOf("invoke('cmd_list_meetings'", helperStart) < panel.indexOf("\n    }\n", helperStart),
  "requestMeetingInventory must issue the cmd_list_meetings request",
);
requireContract(
  panel.includes("if (meetingInventoryFingerprint !== null || documentsViewActive) return;"),
  "loading must be gated to the unresolved meeting inventory",
);
requireContract(
  panel.includes("meetingInventoryLoadingTemplate.cloneNode(true)"),
  "a retry with no rendered inventory must restore the loading state",
);
requireContract(
  panel.includes("Still loading your meetings… (${elapsedSeconds}s)") &&
    panel.includes("Waiting for another Minutes task to finish…"),
  "slow and busy inventory feedback must remain present",
);
requireContract(
  panel.includes("<h2>Couldn't load your meetings</h2>"),
  "the final failure state must describe a load failure",
);

console.log("meeting list loading state: ok");

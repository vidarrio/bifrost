Tracker:AddItems("items/items.json")

-- require: scripts/ first, then the pack root; dots or slashes; init.lua
-- for a folder; one run per module, with its name as `...`.
LOADS = 0
local helper = require("logic.helper")
assert(helper.name == "logic.helper", "module name passed as ...")
assert(require("logic.helper") == helper, "cached")
assert(require("logic/helper") ~= nil)
assert(require("logic.sub").sub == true, "folder init.lua")
assert(require("lib.util").util == true, "pack root fallback")
assert(require("scripts.logic.helper") ~= nil, "pack-root path to scripts/")
assert(require("logic.noreturn") == true, "true for a module returning nothing")
assert(not pcall(require, "logic.missing"), "missing module")
assert(not pcall(require, "..logic.helper"), "empty part")
assert(not pcall(require, "logic/../init"), "traversal")

-- Custom items are found by their codes.
local custom = ScriptHost:CreateLuaItem()
custom.Name = "storage"
custom.ItemState = { seen = 0 }
custom.Icon = ImageReference:FromPackRelativePath("images/x.png")
assert(Tracker:FindObjectForCode("storage") == custom)
local other = ScriptHost:CreateLuaItem()
other.CanProvideCodeFunc = function(self, code) return code == "alias" end
assert(Tracker:FindObjectForCode("alias") == other)
assert(Tracker:FindObjectForCode("nothing") == nil)

assert(Archipelago:GetPlayerGame(1) == nil)
assert(Archipelago:GetPlayerAlias(1) == "")

-- Frame handlers run once after init and after each clear; one that removes
-- itself runs once.
FRAMES = 0
ScriptHost:AddOnFrameHandler("count", function() FRAMES = FRAMES + 1 end)
ScriptHost:AddOnFrameHandler("once", function()
  ScriptHost:RemoveOnFrameHandler("once")
  Tracker:FindObjectForCode("storage").ItemState.seen = 1
end)

-- "*" watches run before every state; removed ones don't.
ScriptHost:AddWatchForCode("all", "*", function()
  local w = Tracker:FindObjectForCode("watched")
  w.AcquiredCount = w.AcquiredCount + 1
end)
ScriptHost:AddWatchForCode("gone", "*", function() error("removed watch ran") end)
ScriptHost:RemoveWatchForCode("gone")

Archipelago:AddClearHandler("clear", function()
  assert(Tracker:FindObjectForCode("storage").ItemState.seen == 1, "frame ran before clear")
  Tracker:FindObjectForCode("key").Active = FRAMES == 1
end)

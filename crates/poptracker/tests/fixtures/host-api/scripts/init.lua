Tracker:AddItems("items/items.json")
Tracker:AddLocations("locations/locations.json")

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
assert(not pcall(require, "logic.cycle"), "a module requiring itself")

-- Custom items are found by the codes their CanProvideCodeFunc accepts (any
-- truthy value; one that fails accepts none), and keep values with :Set/:Get.
local custom = ScriptHost:CreateLuaItem()
custom.Name = "storage"
custom.ItemState = { seen = 0 }
custom.Icon = ImageReference:FromPackRelativePath("images/x.png")
custom.CanProvideCodeFunc = function(self, code) return code == self.Name end
assert(Tracker:FindObjectForCode("storage") == custom)
custom:Set("k", 5)
assert(custom:Get("k") == 5 and custom.ItemState.k == 5)
custom:SetOverlay("x")
local failing = ScriptHost:CreateLuaItem()
failing.CanProvideCodeFunc = function() error("broken") end
local other = ScriptHost:CreateLuaItem()
other.CanProvideCodeFunc = function(self, code) return code == "alias" and 1 end
assert(Tracker:FindObjectForCode("alias") == other)
assert(Tracker:FindObjectForCode("nothing") == nil)
-- A lookup that keeps looking itself up ends, finding nothing.
local loop = ScriptHost:CreateLuaItem()
loop.CanProvideCodeFunc = function(self, code)
  return code == "loop" and Tracker:FindObjectForCode("loop") ~= nil
end
assert(Tracker:FindObjectForCode("loop") == nil)
assert(ImageReference:FromImageReference("a.png", "@disabled") == "a.png")

assert(Archipelago:GetPlayerGame(1) == nil)
assert(Archipelago:GetPlayerAlias(1) == "")

-- Frame handlers run once after init, after each clear and before each state;
-- one that removes itself runs once, and one that fails stops nothing.
local function bump(code)
  local i = Tracker:FindObjectForCode(code)
  i.AcquiredCount = i.AcquiredCount + 1
end
ScriptHost:AddOnFrameHandler("count", function() bump("frames") end)
ScriptHost:AddOnFrameHandler("fails", function() error("frame handler error") end)
ScriptHost:AddOnFrameHandler("once", function()
  ScriptHost:RemoveOnFrameHandler("once")
  bump("once")
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
  Tracker:FindObjectForCode("key").Active = Tracker:FindObjectForCode("frames").AcquiredCount == 1
end)

-- `$name|arg|…` rules call the function with the arguments as strings.
function has_args(a, b)
  return a == "one" and b == "two"
end


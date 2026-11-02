Tracker:AddItems("items.json")
Tracker:AddLocations("locations.json")

local count = function(c) return Tracker:ProviderCountForCode(c) end
local item = function(c) return Tracker:FindObjectForCode(c) end

-- initial_stage_idx, not any other spelling.
local stages = item("s0")
assert(stages.CurrentStage == 1, "initial_stage_idx")
assert(item("wrongkey0").CurrentStage == 0, "initial_stage_index isn't a thing")

-- allow_disabled false: provides by stage alone. Codes inherit, unless told not to.
assert(count("s0") == 1 and count("s1") == 1 and count("s2") == 0)
stages.CurrentStage = 2
assert(count("s2") == 1 and count("s1") == 0 and count("s0") == 0, "inherit_codes false")
stages.CurrentStage = 99
assert(stages.CurrentStage == 2, "the stage is clamped")
stages.CurrentStage = -4
assert(stages.CurrentStage == 0)
stages.Active = false
assert(stages.Active == true, "allow_disabled false stays active")

-- allow_disabled default: nothing until active.
local pt = item("ptoff")
assert(count("ptoff") == 0)
pt.Active = true
assert(count("ptoff") == 1)
pt.Active = false

assert(item("starts").Active == true, "initial_active_state")
assert(item("always").Active == true and count("always") == 1, "static")
item("always").Active = false
assert(item("always").Active == true, "static stays active")

-- Counts are clamped, at the start and on writes; a maximum below the minimum means none.
local coins = item("coins")
assert(coins.AcquiredCount == 5 and coins.MaxCount == 5 and coins.MinCount == 1)
coins.AcquiredCount = 50
assert(coins.AcquiredCount == 5)
coins.AcquiredCount = -50
assert(coins.AcquiredCount == 1)
local unlimited = item("unlimited")
assert(unlimited.MaxCount == -1)
unlimited.AcquiredCount = 1000
assert(unlimited.AcquiredCount == 1000 and unlimited.Active == true)
unlimited.AcquiredCount = 0
assert(unlimited.Active == false, "a consumable is active while it has some")
assert(not pcall(function() coins.AcquiredCount = nil end), "not a number")
coins.AcquiredCount = "3"
assert(coins.AcquiredCount == 3, "a numeric string is a number")

-- Every item providing a code counts; a lookup finds the first.
assert(count("shared") == 5, "summed")
assert(item("shared") == item("a_only"), "the first item with the code")
assert(count("missing") == 0)
assert(item("spaced code") == item("mixed") and item("mixed") ~= nil, "any case, codes with spaces")

-- A composite follows its parts, and they it.
local pair = item("pair")
assert(pair.CurrentStage == 0 and count("pair") == 0)
item("left").Active = true
assert(pair.CurrentStage == 1 and count("pair") == 1)
item("right").Active = true
assert(pair.CurrentStage == 3 and count("pair") == 2)
item("left").Active = false
assert(pair.CurrentStage == 2 and count("pair") == 1)
pair.CurrentStage = 1
assert(item("left").Active == true and item("right").Active == false, "parts follow")
assert(count("left") == 1, "a composite doesn't provide its parts' codes")
item("left").Active = false

-- Fields a pack adds stay its own; what it can't change stays.
coins.MyField = "mine"
assert(coins.MyField == "mine" and coins.Type == "consumable")
assert(type(coins.Owner) == "table")
coins.BadgeText = "text"
assert(coins.BadgeText == "text")
coins:SetOverlay("other")
assert(coins.BadgeText == "other")
coins.Icon = "custom.png"
assert(coins.Icon == "custom.png")
assert(not pcall(function() coins.BadgeText = nil end), "not a string")
assert(not pcall(function() coins:SetOverlay({}) end))
coins.BadgeText = string.rep("x", 100000)
assert(#coins.BadgeText == 4096, "long text is cut")
coins.BadgeText = "other"

-- Numbers stay within an int, so sums can't overflow.
item("shared").AcquiredCount = 1e300
assert(count("shared") > 0)
item("shared").AcquiredCount = 2

-- A part shared by composites moves them both.
item("left2").Active = true
assert(item("pair2").CurrentStage == 1 and item("pair3").CurrentStage == 1)
item("pair2").CurrentStage = 0
assert(item("left2").Active == false and item("pair3").CurrentStage == 0, "through the part")

-- Custom items count toward codes, as their own functions say.
local custom = ScriptHost:CreateLuaItem()
custom.CanProvideCodeFunc = function(self, code) return code == "custom_code" or code == "shared" end
custom.ProvidesCodeFunc = function(self, code) return 4 end
assert(count("custom_code") == 4)
assert(count("shared") == 9, "with the JSON items")
custom.CanProvideCodeFunc = function() return true end
custom.ProvidesCodeFunc = function(self, code) return count(code) end
assert(count("left") == 0, "one asking in turn ends")
-- They get the code as written, and PotentialCodes comes first.
custom.CanProvideCodeFunc = function(self, code) return code == "MyCode" end
custom.ProvidesCodeFunc = function() return 2 end
assert(count("MyCode") == 2 and count("mycode") == 0, "as written")
custom.PotentialCodes = { "Listed" }
assert(count("Listed") == 2 and count("MyCode") == 0)
custom.PotentialCodes = nil
custom.ProvidesCodeFunc = function() return 1e300 end
assert(count("MyCode") ~= 0, "a huge answer doesn't overflow")
custom.ProvidesCodeFunc = function(self) return self:Get("n") or 0 end
custom.CanProvideCodeFunc = function(self, code) return code == "MyCode" end

-- A level read right after a change sees it, in the same handler.
Archipelago:AddClearHandler("levels", function()
  local gate = Tracker:FindObjectForCode("@Gate/S")
  item("left").Active = false
  item("right").Active = false
  assert(gate.AccessibilityLevel == AccessibilityLevel.None, "blocked")
  item("left").Active = true
  item("right").Active = true
  assert(gate.AccessibilityLevel == AccessibilityLevel.Normal, "open, straight away")
  item("left").Active = false
  assert(gate.AccessibilityLevel == AccessibilityLevel.None, "blocked again")
  -- A custom item's Set shows too.
  local viaCustom = Tracker:FindObjectForCode("@ViaCustom/S")
  assert(viaCustom.AccessibilityLevel == AccessibilityLevel.None, "custom not set")
  custom:Set("n", 1)
  assert(viaCustom.AccessibilityLevel == AccessibilityLevel.Normal, "custom set")
  custom:Set("n", 0)
  -- A hosted section has no chests, and is collected when its item is held.
  local boss = Tracker:FindObjectForCode("@Boss/Reward")
  assert(boss.ChestCount == 0 and boss.AccessibilityLevel == AccessibilityLevel.Normal, "boss not held")
  item("left").Active = true
  assert(boss.AccessibilityLevel == AccessibilityLevel.Cleared, "boss held")
  item("left").Active = false
  -- Hosted codes reach a custom item as written.
  custom.CanProvideCodeFunc = function(self, code) return code == "HostedCode" end
  custom.ProvidesCodeFunc = function() return 1 end
end)

-- Watches hear of what changed, once the operation that changed it is done.
HEARD = {}
ScriptHost:AddWatchForCode("single", "starts", function(code) HEARD[code] = (HEARD[code] or 0) + 1 end)
ScriptHost:AddWatchForCode("any case", "MIXED", function(code)
  HEARD[code] = true
  item("coins").AcquiredCount = 4
end)
ScriptHost:AddWatchForCode("set off", "coins", function(code) HEARD.chained = code end)
ScriptHost:AddWatchForCode("not this", "always", function() error("unchanged") end)
ScriptHost:AddWatchForCode("wild", "*", function(codes) if codes == "starts" then HEARD.wild = codes end end)
ScriptHost:AddWatchForCode("part", "pair", function() HEARD.pair = true end)
ScriptHost:AddWatchForCode("custom", "ByCustom", function(code) HEARD[code] = true end)
ScriptHost:AddWatchForCode("custom wild", "*", function(codes)
  if codes == "byCustomName" then HEARD.customWild = true end
  if codes == "bystander" then error("this one didn't change") end
end)
-- A watch may remove itself, and one that keeps changing things is stopped.
HEARD.once = 0
ScriptHost:AddWatchForCode("once", "starts", function()
  HEARD.once = HEARD.once + 1
  ScriptHost:RemoveWatchForCode("once")
end)
HEARD.flips = 0
ScriptHost:AddWatchForCode("flip", "unlimited", function()
  HEARD.flips = HEARD.flips + 1
  local u = item("unlimited")
  u.AcquiredCount = u.AcquiredCount + 1
end)
local bystander = ScriptHost:CreateLuaItem()
bystander.Name = "bystander"
bystander.CanProvideCodeFunc = function(self, code) return code == "NotThisOne" end
ScriptHost:AddWatchForCode("custom other", "NotThisOne", function() error("this one didn't change") end)
local byCustom = ScriptHost:CreateLuaItem()
byCustom.CanProvideCodeFunc = function(self, code) return code == "ByCustom" end
byCustom.Name = "byCustomName"
Archipelago:AddClearHandler("change", function()
  HEARD.flips = 0
  item("starts").Active = false
  item("mixed").Active = true
  item("left").Active = true
  item("left").Active = false
  byCustom:Set("v", 1)
  item("unlimited").AcquiredCount = 1
  assert(HEARD.starts == nil, "not during the handler")
end)
Archipelago:AddItemHandler("heard", function()
  assert(HEARD.starts == 1, "a watch on its code")
  assert(HEARD.wild == "starts", "a wildcard gets the item's codes")
  assert(HEARD.MIXED == true, "any case, given as the pack wrote it")
  assert(HEARD.chained == "coins", "a watch's change sets off others")
  assert(HEARD.pair == true, "a composite through its part")
  assert(HEARD.ByCustom == true, "a custom item's Set")
  assert(HEARD.customWild == true, "a wildcard hears which custom item")
  assert(HEARD.once == 1, "removed, it isn't told again")
  assert(HEARD.flips > 0 and HEARD.flips <= 8, "stopped after a few rounds")
  Tracker:FindObjectForCode("always").Active = true
  HEARD.checked = true
  item("level_ok").Active = true
end)

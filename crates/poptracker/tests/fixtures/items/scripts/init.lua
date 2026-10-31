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
assert(item("spaced code").Name == "MiXeD" and item("mixed").Name == "MiXeD", "any case, codes with spaces")

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
assert(coins.MyField == "mine" and coins.Type == "consumable" and coins.Name == "Coins")
assert(type(coins.Owner) == "table")
coins.BadgeText = "text"
assert(coins.BadgeText == "text")
coins:SetOverlay("other")
assert(coins.BadgeText == "other")
coins.Icon = "custom.png"
assert(coins.Icon == "custom.png")

-- Custom items count toward codes, as their own functions say.
local custom = ScriptHost:CreateLuaItem()
custom.CanProvideCodeFunc = function(self, code) return code == "custom_code" or code == "shared" end
custom.ProvidesCodeFunc = function(self, code) return 4 end
assert(count("custom_code") == 4)
assert(count("shared") == 9, "with the JSON items")
custom.CanProvideCodeFunc = function() return true end
custom.ProvidesCodeFunc = function(self, code) return count(code) end
assert(count("left") == 0, "one asking in turn ends")
custom.CanProvideCodeFunc = function() return false end

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
  -- A hosted section has no chests, and is collected when its item is held.
  local boss = Tracker:FindObjectForCode("@Boss/Reward")
  assert(boss.ChestCount == 0 and boss.AccessibilityLevel == AccessibilityLevel.Normal, "boss not held")
  item("left").Active = true
  assert(boss.AccessibilityLevel == AccessibilityLevel.Cleared, "boss held")
  item("left").Active = false
end)

Tracker:AddItems("items.json")
Tracker:AddLocations("locations.json")
Tracker:FindObjectForCode("have").Active = true
Tracker:FindObjectForCode("three").AcquiredCount = 3

assert(AccessibilityLevel.None == 0 and AccessibilityLevel.Partial == 1
  and AccessibilityLevel.Inspect == 3 and AccessibilityLevel.SequenceBreak == 5
  and AccessibilityLevel.Normal == 6 and AccessibilityLevel.Cleared == 7)

function yes() return true end
function no() return false end
function fails() error("in a rule") end
function args_two(a, b) return a == "one" and b == "two" end
function normal() return AccessibilityLevel.Normal end
function sequence() return AccessibilityLevel.SequenceBreak end
function inspect() return AccessibilityLevel.Inspect end
function none() return AccessibilityLevel.None end
function three_fn() return 3 end
function echo_num(n) return tonumber(n) end
function num_string() return "2" end
function helper_open()
  return Tracker:FindObjectForCode("@Helper/Open").AccessibilityLevel == AccessibilityLevel.Normal
end

-- Objects are found as in rules, and read their level from them.
Archipelago:AddClearHandler("objects", function()
  local section = Tracker:FindObjectForCode("@Helper/Shut")
  local location = Tracker:FindObjectForCode("@Container")
  local byName = Tracker:FindObjectForCode("@Inner")
  local single = Tracker:FindObjectForCode("@Have")
  assert(section.AccessibilityLevel == AccessibilityLevel.None, "section")
  assert(section.FullID == "Helper/Shut" and section.ChestCount == 1)
  assert(location.AccessibilityLevel == AccessibilityLevel.Normal, "location")
  assert(location.ChestCount == nil)
  assert(byName.FullID == "Outer/Inner")
  assert(single == Tracker:FindObjectForCode("@Have/S"), "one section means that section")
  assert(Tracker:FindObjectForCode("@Nowhere") == nil)
  -- Counts are live, and a collected section reads as cleared.
  local miss = Tracker:FindObjectForCode("miss")
  miss.Active = true
  assert(Tracker:ProviderCountForCode("miss") == 1, "sees its own change")
  miss.Active = false
  assert(Tracker:ProviderCountForCode("miss") == 0)
  local clearing = Tracker:FindObjectForCode("@ToClear/S")
  clearing.AvailableChestCount = 0
  assert(clearing.AccessibilityLevel == AccessibilityLevel.Cleared, "cleared")
  Tracker:FindObjectForCode("@Shop/Sold").AvailableChestCount = 1
  -- A ref holds no chests of its own, so reads as cleared.
  local here = Tracker:FindObjectForCode("@ShopSeenHere/Sold")
  assert(here.ChestCount == 0 and here.AccessibilityLevel == AccessibilityLevel.Cleared, "ref object")
  Tracker:FindObjectForCode("level_seen").Active = true
end)

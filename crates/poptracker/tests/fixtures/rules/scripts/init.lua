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
  Tracker:FindObjectForCode("level_seen").Active = true
end)

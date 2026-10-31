assert(Tracker.ActiveVariantUID == "map")
ScriptHost:LoadScript("scripts/encoded.lua")
assert(FROM_ROOT)
require("only_here")
assert(FROM_VARIANT, "require searches the variant")
Tracker:AddItems("items.json")

-- The small os, however it's reached.
assert(os.time() > 0 and os.date("%Y") and os.clock() >= 0 and os.difftime(2, 1) == 1)
assert(os.execute == nil and os.remove == nil and os.getenv == nil and os.exit == nil)
assert(require("os") == os and require("string") == string and require("utf8") == utf8)
assert(not pcall(require, "io") and not pcall(require, "debug") and not pcall(require, "package"))

-- Lookups of nothing find nothing.
assert(ScriptHost:LoadScript(nil) == false)
assert(Tracker:FindObjectForCode(nil) == nil and Tracker:ProviderCountForCode(nil) == 0)

-- API that only needs not to fail here.
assert(Archipelago.ClientStatus.GOAL == 30 and Archipelago:GetItemName(1) == "Unknown")
ScriptHost:AsyncProgress(1)
local ran
ScriptHost:RunStringAsync("return arg + 1", 41, function(v) ran = v end)
assert(ran == 42 and arg == nil)
local failed = false
ScriptHost:RunStringAsync("error('x')", nil, function() failed = true end)
assert(not failed, "a failed script doesn't complete")
ran = nil
ScriptHost:RunScriptAsync("scripts/encoded.lua", nil, function() ran = true end)
assert(ran)

Archipelago:AddClearHandler("first", function() error("broken handler") end)
Archipelago:AddClearHandler("second", function()
  local i = Tracker:FindObjectForCode("after_error")
  i.Active = true
  i:SetOverlay("hello")
  i:SetOverlayColor("#fff")
  i:SetOverlayBackground("#000")
  i:SetOverlayFontSize(12)
  i:SetOverlayAlign("left")
end)
Archipelago:AddItemHandler("bad", function() error("broken too") end)
Archipelago:AddItemHandler("good", function() Tracker:FindObjectForCode("got_item").Active = true end)
Archipelago:AddLocationHandler("spin", function() while true do end end)

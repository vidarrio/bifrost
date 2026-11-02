Tracker:AddItems("items.json")
Tracker:AddMaps("maps.json")
local icon = Tracker:FindObjectForCode("icon")
icon.Icon = "custom.png:dim"
assert(icon.Icon == "custom.png:dim")
Archipelago:AddClearHandler("c", function() end)

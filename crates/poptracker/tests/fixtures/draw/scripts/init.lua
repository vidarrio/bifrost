Tracker:AddItems("items.json")
Tracker:AddMaps("maps.json")
local icon = Tracker:FindObjectForCode("icon")
icon.Icon = "custom.png"
icon.IconMods = "dim"
Archipelago:AddClearHandler("c", function() end)

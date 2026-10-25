function has_enough_b()
    local b = Tracker:FindObjectForCode("b")
    if b and b.AcquiredCount >= 3 then return 3 end -- AccessibilityLevel.Reachable
    return 0 -- AccessibilityLevel.None
end

// ============================================================================
// Skill Studio marketing demo - detail navigation labels
// ============================================================================

export type ProductMockRootView = "home" | "skills" | "plugins" | "activity";

/** Returns the root-view label shown by ProductMock's detail back button. */
export function productMockDetailBackLabel(from: ProductMockRootView): string {
  switch (from) {
    case "home":
      return "Home";
    case "skills":
      return "Skills";
    case "plugins":
      return "Plugins";
    case "activity":
      return "Activity";
  }
}

// ============================================================================
// @skill-studio/ui - Public entry point
// Re-exports the shadcn-style Base UI kit. Import styles separately from
// "@skill-studio/ui/styles" once, from the app's CSS entry.
// ============================================================================

export { cn } from "./lib/cn";

export { Button, buttonVariants } from "./components/button";
export { Badge, badgeVariants } from "./components/badge";
export {
  Card,
  CardHeader,
  CardFooter,
  CardTitle,
  CardAction,
  CardDescription,
  CardContent,
} from "./components/card";
export { Input } from "./components/input";
export { Textarea } from "./components/textarea";
export { Checkbox } from "./components/checkbox";
export { Switch } from "./components/switch";
export {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectScrollDownButton,
  SelectScrollUpButton,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "./components/select";
export { Tabs, TabsList, TabsTrigger, TabsContent, tabsListVariants } from "./components/tabs";
export { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "./components/tooltip";
export {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
} from "./components/popover";
export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
} from "./components/dialog";
export {
  DropdownMenu,
  DropdownMenuPortal,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuItem,
  DropdownMenuCheckboxItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
} from "./components/dropdown-menu";
export { Separator } from "./components/separator";
export { ScrollArea, ScrollBar } from "./components/scroll-area";

export { KitPreview } from "./kit-preview";

// ============================================================================
// @skill-studio/ui - KitPreview
// Renders one of every component in the kit, for a visual smoke test. Not
// mounted by default; the desktop app mounts it behind a dev-only route
// (see apps/desktop/src/App.tsx).
// ============================================================================

import { useState } from "react";
import { Bell, Settings } from "lucide-react";

import { Badge } from "./components/badge";
import { Button } from "./components/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "./components/card";
import { Checkbox } from "./components/checkbox";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "./components/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./components/dropdown-menu";
import { Input } from "./components/input";
import { Popover, PopoverContent, PopoverTrigger } from "./components/popover";
import { ScrollArea } from "./components/scroll-area";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./components/select";
import { Separator } from "./components/separator";
import { Switch } from "./components/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./components/tabs";
import { Textarea } from "./components/textarea";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./components/tooltip";

export function KitPreview() {
  const [scope, setScope] = useState("global");

  return (
    <TooltipProvider>
      <div className="flex min-h-screen flex-col gap-6 bg-background p-8 text-foreground">
        <h1 className="text-2xl font-semibold">Skill Studio UI Kit</h1>

        <div className="flex flex-wrap items-center gap-3">
          <Button>Default</Button>
          <Button variant="outline">Outline</Button>
          <Button variant="secondary">Secondary</Button>
          <Button variant="ghost">Ghost</Button>
          <Button variant="destructive">Destructive</Button>
          <Button variant="link">Link</Button>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Badge>Default</Badge>
          <Badge variant="secondary">Secondary</Badge>
          <Badge variant="destructive">Destructive</Badge>
          <Badge variant="outline">Outline</Badge>
        </div>

        <Card className="max-w-sm">
          <CardHeader>
            <CardTitle>Installed skill</CardTitle>
            <CardDescription>skills.sh / commit-message-writer</CardDescription>
            <CardAction>
              <Settings size={16} />
            </CardAction>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <Input placeholder="Search skills" />
            <Textarea placeholder="Notes" />
            <label className="flex items-center gap-2 text-sm">
              <Checkbox />
              Enable for this project
            </label>
            <label className="flex items-center gap-2 text-sm">
              <Switch />
              Auto-update
            </label>
            <Select value={scope} onValueChange={(next) => next != null && setScope(next)}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="global">Global</SelectItem>
                <SelectItem value="project">Project</SelectItem>
              </SelectContent>
            </Select>
          </CardContent>
          <CardFooter className="justify-end gap-2">
            <Button variant="outline" size="sm">
              Cancel
            </Button>
            <Button size="sm">Save</Button>
          </CardFooter>
        </Card>

        <Tabs defaultValue="browse">
          <TabsList>
            <TabsTrigger value="browse">Browse</TabsTrigger>
            <TabsTrigger value="installed">Installed</TabsTrigger>
          </TabsList>
          <TabsContent value="browse">Browse panel content.</TabsContent>
          <TabsContent value="installed">Installed panel content.</TabsContent>
        </Tabs>

        <Separator />

        <div className="flex flex-wrap items-center gap-3">
          <Tooltip>
            <TooltipTrigger render={<Button variant="outline" size="icon" />}>
              <Bell size={16} />
            </TooltipTrigger>
            <TooltipContent>Notifications</TooltipContent>
          </Tooltip>

          <Popover>
            <PopoverTrigger render={<Button variant="outline" />}>Popover</PopoverTrigger>
            <PopoverContent>Popover content lives here.</PopoverContent>
          </Popover>

          <DropdownMenu>
            <DropdownMenuTrigger render={<Button variant="outline" />}>Menu</DropdownMenuTrigger>
            <DropdownMenuContent>
              <DropdownMenuLabel>Actions</DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem>Edit</DropdownMenuItem>
              <DropdownMenuItem variant="destructive">Remove</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          <Dialog>
            <DialogTrigger render={<Button variant="outline" />}>Open dialog</DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Remove skill</DialogTitle>
                <DialogDescription>This can't be undone.</DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
                <Button variant="destructive">Remove</Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </div>

        <ScrollArea className="h-32 max-w-sm rounded-lg border border-border p-3">
          {Array.from({ length: 20 }, (_, i) => (
            <p key={i} className="py-1 text-sm text-muted-foreground">
              Scrollable row {i + 1}
            </p>
          ))}
        </ScrollArea>
      </div>
    </TooltipProvider>
  );
}

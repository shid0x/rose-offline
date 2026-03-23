
# Slop rose

This version of rose-online is based on the [@rose-offline](https://www.github.com/exjam/rose-offline) version, made by the awesome  [@exjam](https://www.github.com/exjam)

This project does not have any particular ambition. It is mostly an experiment to see how far current AI models can take us when it comes to Rose Online development.

The models used in this project were Opus 4.6, codex 5.3 and later GPT 5.4 . All proved capable of handling complex tasks and the “voodoo Korean magic” code from the early 2000s.

Some fixes took a noticeable amount of time and required doing your own research while guiding the AI along the way. Others were solved almost instantly in a single attempt.


## Features
Note: This list should be compared with rose-offline, as this project expands on it.

**New content:**

- Skill leveling system (the ability to level up skills)
- Shop system implemented
- Clan system implemented (the original UI was scrapped and replaced with a new EGUI-based clan management window; clans can be created with the usual NPC or with `/clan create XXX`)
- Party system fully implemented (party levels, item distribution; some formulas were simplified)
- Refine system implemented
- Craft system implemented
- Item disassembly implemented
- Many different quest types are now supported (this fixes the cart quest and some of the episode quests)
- PVP (training grounds and clan fields)
- Summon gauge implemented
- Bonfire implemented (they were also slightly buffed, as iROSE bonfires are not very strong; reminder: iROSE bonfires affect party members only)
- Chat bubbles implemented
- Add friends and private chat implemented
- Refine glow visible
- Stealth implemented
- Clan field monsters spawns

**Extra features**

- 2047 damage limit removed
- `/mon ID vs ID` (useless but fun)
- Anti-aliasing and anti-shimmering measures (to use these, you can either use the pre-generated client provided in the release or run the mipgen tool and generate the mipmaps yourself — see below)
- Added a tooltip window that appears when hovering over character statistics


**Bug fixes (bugs in rose-offline that have been corrected)**

- Clicking the exit button now closes the game correctly
- You can now drag and drop items when buying
- Players no longer leave combat after using a skill
- Players no longer chain “miss” after using a skill
- Player nameplates now follow when players enter carts or castle gear
- Various UI quirks were fixed (for example, items are now grayed out when selling)
- You can now equip gems on socketed items
- **The collider map has been fixed** (no more invisible tower in the middle of Junon; you can climb Luxem, etc.)
- AOE skills now show death animations, damage counters, and sounds
- Luna teleporter and various CON-driven events are now handled (such as the mushrooms in the episode quest)
- Union and clan are displayed correctly in the Character tab
- PVP QSDs are now handled correctly (these QSDs allow you to leave the clan field / crusader training ground)
- RewardMoney now replaces the local money total instead of adding to it
- Quest icons are now visible
- Giving up quests is now possible
- Refining and disassembly at NPCs are now possible with the correct formula (money instead of MP)
- Corrected the clan house teleport point (the original QSD had out-of-bounds coordinates)
- Poison is now visible in both PVE and PVP
- Prevents characters from equipping an off-hand item while a two-handed weapon is already equipped. Credits : [@Yentis](https://www.github.com/Yentis
- Resolved an issue where summoned entities could incorrectly take ownership of items. Credit : [@Yentis](https://www.github.com/Yentis
- Improved tooltip text Credits : [@Yentis](https://www.github.com/Yentis)

## How to use the mipmaps tool

- You need to extract the mipmaps into a folder (called `rose mips` in this example).
- This can be done by running the mipgen tool provided (PowerShell):

```powershell 
& "c:\FOLDER\rose-dds-mipgen.exe" `
  --input-path "c:\YOURCLIENT\data.idx" `
  --vfs-type rose `
  --output-path "c:\YOURCLIENT\rose mips" `
  --discovery-scope full `
  --filter kaiser `
  --report-json "c:\YOURCLIENT\mipgen-gate2.json"
  ```

The JSON report will tell you what succeeded and what failed.

1 — Once this is done, all you have to do is set up your config.toml correctly:
```
[filesystem]
devices = [
    { type = "vfs", path = "C:\\YOURCLIENT\\data.idx" },
    { type = "directory", path = "C:\\YOURCLIENT\\rose mips" },
]

[server]
ip = "127.0.0.1"
port = 29000

[graphics]
anti_aliasing = "msaa"

```
Note: You must use double `\\` in the TOML configuration.

You can change the AA setting. FXAA works but is fairly experimental; the current setup works best for anti-aliasing and shimmering. You may also notice that it uses a Kaiser filter for downsampling, which gave the best results.

## Known issues

- Item appraisal is not implemented
- Item lifespan is not implemented (mainly because it is a shitty mechanic )
- Some sound effects are not linked to their actions
- Flying ships are not visible when changing planets
- Event items (Christmas tree) are not implemented, mainly because they are hardcoded and tedious to add one by one
- Some monsters walk (yes); when chasing you, they move at a slower speed. Never tried to fix it — it looked fun.
- Figthing under bonfires can look a little strange : Server side : Healed before receiving the hit, Client side : Healed after receiving the hit. So you might see yourself at 1HP for a second if you were low on HP.



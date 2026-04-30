# ROSE NPC Shop Editor

This program lets you view and edit what shop NPCs sell.

## What It Can Do

- Open shop data from a packed `data.idx` file.
- Open shop data from a pre-extracted data folder.
- Let you choose which mode to use when the program starts.
- Show a list of NPCs that actually sell items.
- Let you find shop NPCs by zone.
- Show the shop tabs used by the selected NPC.
- Show the items sold in each tab.
- Add items to a shop tab.
- Remove items from a shop tab.
- Help you find items with search and category filters.
- Show the item icon next to the item name.
- Save changes back to the source you opened:
  - If you opened `data.idx`, it saves back to the packed files.
  - If you opened an extracted folder, it saves back to the extracted files.
- Create backup files when it saves changes.
- Reload the data if you want to discard what is currently open and read it again.

## A Few Helpful Notes

- Changes are made to the selected NPC entry, not just one single spawn in the world.
- If a shop tab is shared by more than one NPC, the editor protects the other NPCs by making a separate copy before changing it.
- The program is made to be a practical tool for browsing, checking, and editing NPC shops without digging through the raw files by hand.

## Footnote

The item icons were a little tricky to get working because the game does not store them as one simple list of image files.

The editor first reads `ITEM1.TSI`, which acts like a map for the icon sheets. It tells the program which texture file to use and which rectangle inside that texture belongs to each icon. After that, the editor loads the referenced texture file, which is often a DDS image such as `ICON01.DDS`.

The difficult part is that many of these DDS files use older pixel formats that modern image loaders do not always handle well. So the editor does a bit of extra work: it reads the DDS header, detects the real format being used, decodes the image data into normal RGBA pixels, and then cuts out the correct icon area based on the coordinates stored in the TSI file.

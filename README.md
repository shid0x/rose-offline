
# Slop rose

This version of rose-online is based on the [@rose-offline](https://www.github.com/exjam/rose-offline) version, made by the awesome  [@exjam](https://www.github.com/exjam)

This project does not have any particular ambition. It is mostly an experiment to see how far current AI models can take us when it comes to Rose Online development.

The models used in this project were Opus 4.6, codex 5.3 and later GPT 5.4 . All proved capable of handling complex tasks and the “voodoo Korean magic” code from the early 2000s.

Some fixes took a noticeable amount of time and required doing your own research while guiding the AI along the way. Others were solved almost instantly in a single attempt.

## Setup and client :

Recommended client for Slop Rose can be found here : [Slop Rose client](https://mega.nz/file/kYk0SIrL#dyt9mN9eHFz-7aol5a2BH7EyhDv_MuKElxQEbazbul8) , This client include pre extracted mips folder
so you don't need to do it yourself

Edit config.toml with your desired settings  ( you have to point it toward your local data.idx and the mips folder ) , then launch the server and the client.


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
- Flying ships are not visible when changing planets
- Event items (Christmas tree) are not implemented, mainly because they are hardcoded and tedious to add one by one
- Some monsters walk (yes); when chasing you, they move at a slower speed. Never tried to fix it — it looked fun.
- Figthing under bonfires can look a little strange : Server side : Healed before receiving the hit, Client side : Healed after receiving the hit. So you might see yourself at 1HP for a second if you were low on HP.



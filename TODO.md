# TODO

## Verify by playthrough
- [x] 1. Save drops inventory beyond 16 slots? No: UI has 16 slots (m1–m16), save holds 16. The `m17` warning is a harmless off-by-one in frame 1984/1985 loops.
- [x] 2. Loading a save restores `english` language choice correctly. (headless_save_load_autosave)
- [x] 3. Music never doubles/restarts on scene changes. (Same track requested again keeps playing; the game asks twice for menu.mid.)
- [ ] 4. Ruffle Flash 5 quirks: dead hotspots, broken animations, stuck items — patch AS where needed.

## Easy improvements
- [x] 5. Best render quality, crisp vectors at any size.
- [x] 6. Multiple save slots + autosave on quit.
- [x] 7. Settings outside the save file (music volume, fullscreen, quality).
- [x] 8. Hotspot highlight key.
- [x] 9. Custom SoundFont support (e.g. converted Windows GS set) over bundled GeneralUser GS.

## Distribution
- [ ] 10. Release builds: macOS universal .app, Windows .exe, Linux AppImage, web build.
- [x] 11. Import straight from `32-ZKinstall.exe` (RAR SFX) without unar.

## Content
- [x] 12. Measure and complete the English translation. (Script text was 100% bilingual; 2 drawn texts added.)
- [x] 13. Text extract/re-import so dialogue can be edited as plain text.

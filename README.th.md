# SSH Control Center

[English](README.md) | **ไทย**

> **โปรเจกต์โชว์ผลงานส่วนตัว เวอร์ชัน 0.4.0 ยังไม่ผ่านการตรวจสอบความปลอดภัยและยังไม่พร้อมใช้งานจริง (not production-ready) ลองบนเครื่องเสมือนที่ทิ้งได้เท่านั้น**

รีโมตคอนโทรลสำหรับกลุ่มเครื่องพีซีและเซิร์ฟเวอร์ขนาดเล็ก: บอกว่าอยากให้ทำอะไร แล้วงานจะเกิดขึ้นบนเครื่องที่ถูกต้อง แม้เครือข่ายจะกะพริบ

*ภาพเปรียบเทียบในชีวิตประจำวัน* ลองนึกถึงผู้จัดการอาคารที่ถือพวงกุญแจ คุณบอกว่า "ช่วยเช็กห้อง 12 หน่อย" ผู้จัดการเดินไปทำให้ แล้วจดผลลงสมุดบันทึกของห้องนั้น ต่อให้คุณวางสายโทรศัพท์ไปแล้ว งานก็ยังเสร็จ และสมุดบันทึกก็ยังอยู่ ในเครื่องมือนี้ ผู้จัดการคือ **Master** (เครื่องที่คุณนั่งทำงาน) ห้องต่าง ๆ คือ **Target** (เครื่องที่คุณควบคุม) และสมุดบันทึกคือ log ของงานที่เก็บไว้บน Target

SSH Control Center ให้คุณ (หรือ AI agent) สั่งงาน แก้ไฟล์ และเปิด terminal บนเครื่อง Windows และ Ubuntu ทุกเครื่องผ่าน SSH ได้จากที่เดียว ออกแบบไว้สำหรับเครื่อง Windows และ Ubuntu 24.04 แบบ 64 บิต (x64) โดย Master หนึ่งเครื่องมี Target ได้สูงสุด 50 เครื่อง

![มุมมองรวมของแอป desktop พร้อมเครื่องตัวอย่างหกเครื่อง](docs/images/fleet-mockup.png)

*ภาพจำลองเพื่อประกอบคำอธิบาย ข้อมูลทั้งหมดเป็นข้อมูลสมมติ (Illustrative mockup with fake data) หน้าจอของแอปจริงเป็นภาษาไทย*

## คืออะไร และทำไมถึงมี

- **คืออะไร.** โปรแกรมเล็ก ๆ ที่เขียนด้วย Rust ชื่อ `sshcc` พร้อมแอปบน tray ให้เลือกใช้เพิ่มได้ SSH คือวิธีมาตรฐานที่เข้ารหัสไว้ ให้คอมพิวเตอร์เครื่องหนึ่งควบคุมอีกเครื่องหนึ่ง เครื่องที่คุณนั่งทำงานคือ **Master** (เครื่องหลัก) เครื่องที่ถูกควบคุมคือ **Target** (เครื่องปลายทาง) ส่วน **job** (งาน) คือคำสั่งหนึ่งชุดที่คุณขอให้ Target รัน
- **ทำไม.** การดูแลหลายเครื่องมักหมายถึงต้องมีหลายบัญชี หลายกุญแจ และหลายหน้าต่าง terminal ที่นี่มีที่เดียวที่รู้จักทุกเครื่อง และ AI agent ก็ได้สิทธิ์เข้าถึงแบบเดียวกันผ่าน **MCP** (มาตรฐานกลางที่ให้ผู้ช่วย AI ใช้เครื่องมือภายนอก)
- **ออกแบบมาสำหรับการเชื่อมต่อที่ไม่นิ่ง.** งานแต่ละงานมี ID ถ้าการเชื่อมต่อหลุด งานจะยังรันต่อบน Target และคุณกลับมาอ่านผลได้ภายหลัง ส่งงานเดิมซ้ำก็ไม่ทำให้งานรันสองรอบ ส่วนนี้ทดสอบด้วยการจำลองการหลุด (รีสตาร์ต container และ process) ส่วนกรณี WAN จริง การรีบูต และ logout ยังไม่ได้ทดสอบ
- **เหมาะกับใคร.** คนที่ดูแลเครื่อง Windows หรือ Ubuntu ของตัวเองจำนวนไม่มาก และอยากสั่งงานทั้งหมดจากที่เดียว จะสั่งเองหรือสั่งผ่าน AI agent ก็ได้ เหมาะกับการลองใช้ เรียนรู้ และอ่านโค้ด ยังไม่เหมาะกับ: เครื่อง Mac หรือคนที่ต้องการผลิตภัณฑ์ที่ลงลายเซ็นดิจิทัลและผ่านการตรวจความปลอดภัยโดยผู้ตรวจอิสระ ([FAQ](docs/th/faq.md#2-ใครควรใช้))

## ทำงานอย่างไร

```mermaid
flowchart LR
  A["คุณหรือ AI agent"] --> B["Master service"]
  B -->|"SSH"| C["เครื่อง Target"]
  C --> D["log ของงานเก็บไว้บน Target"]
```

*พูดง่าย ๆ: คุณสั่ง Master, Master ไปหา Target ผ่าน SSH และ Target เป็นผู้เก็บบันทึกไว้*

อ่านทีละขั้นได้ที่ [How it works (ภาษาไทย)](docs/th/how-it-works.md) สำหรับนักพัฒนา: [สถาปัตยกรรม](docs/th/architecture.md)

<details>
<summary>สำหรับนักพัฒนา: ไฟล์อยู่ตรงไหน</summary>

- `crates/cli`: คำสั่ง `sshcc` (`init`, `service`, `mcp`, `shell`, `pairing`, `doctor`, `exec`, `job` และอื่น ๆ)
- `crates/core`: service, การจัดการ SSH, jobs, ไฟล์, terminal, pairing และเครื่องมือ MCP
- `desktop`: tray (Tauri 2 บน Windows, GTK/AppIndicator บน Ubuntu) และหน้าต่างจัดการที่เปิดเมื่อเรียกใช้ ด้วย Tauri 2 หน้า UI เป็น HTML, CSS และ JS ธรรมดา ไม่มีขั้นตอน build ฝั่ง frontend ([desktop/README.md](desktop/README.md))
- `installers`, `scripts`, `tests`, `docs`: ตัวติดตั้ง Windows และ Ubuntu, สคริปต์ build และ smoke, การทดสอบ, เอกสาร

</details>

## ความสามารถ

- **งานที่ทนต่อการหลุด (durable jobs).** รันสคริปต์บน Target เบื้องหลัง งานยังทำต่อแม้ SSH หลุด ถ้า service ของ Target รีสตาร์ต งานที่ยังไม่จบจะถูกทำเครื่องหมาย `interrupted` และไม่ถูกรันซ้ำโดยอัตโนมัติ
- **เข้าถึงไฟล์อย่างระมัดระวัง.** ดูรายการ อ่าน ค้นหา แก้ไข และเขียนไฟล์ได้ การเขียนจะแทนที่ทั้งไฟล์ในครั้งเดียว ปฏิเสธ symlink และแนบการตรวจ SHA-256 ได้ เพื่อไม่ให้ทับงานที่คนอื่นเพิ่งแก้ การเขียนหนึ่งครั้งจำกัดที่ 4 MiB
- **Terminal แบบโต้ตอบ.** ใช้ `sshcc shell <Target>` หรือเครื่องมือ terminal (ต้องใช้โหมด Owner session ดูด้านล่าง) กด Ctrl+] เพื่อ detach แล้วกลับมาต่อด้วย terminal ID ภายใน 30 นาทีหลังใช้ครั้งล่าสุด ถ้า service รีสตาร์ตหรือ Owner logout terminal จะสิ้นสุด และรันพร้อมกันได้สูงสุด 8 อันต่อ Target ข้อมูลที่แสดงเก็บในหน่วยความจำขนาด 4 MiB และสิ่งที่พิมพ์เข้าไปจะไม่ถูกเก็บลงดิสก์
- **MCP server สำหรับ AI agent.** `sshcc mcp` คุย MCP ผ่าน stdio เครื่องมือครอบคลุม targets, jobs, ไฟล์, terminal, pairing และตัวช่วยเขียนโค้ดอย่าง `exec_run`, `files_grep` และ `files_edit`
- **ตั้งค่า AI client.** ปุ่ม "เชื่อม" ในแอป desktop เพิ่มรายการ `ssh_control_center` ให้ Cursor, Claude Code หรือ Codex และเพิ่มให้เฉพาะเมื่อยังไม่มีรายการนี้ ถ้ามีรายการอยู่แล้วแต่ path ที่เก็บไว้ไม่ตรง จะมีปุ่ม "อัปเดต path" แยกต่างหากที่เขียน command และ args ของรายการนั้นใหม่ หลังสำรองไฟล์ตั้งค่าพร้อมตราเวลา ([ADR 0001](docs/adr/0001-mcp-registration-no-overwrite.md))
- **จับคู่เครื่องใน 5 ขั้นตอน.** กรอกข้อมูลเครื่องใหม่ที่ Master, บันทึกชุดติดตั้ง, เปิดที่ Target แล้วอนุมัติ, คัดลอกรหัสจับคู่ที่ Target แสดง แล้ววางรหัสกลับที่ Master ไม่ต้องจัดการ SSH key หรือไฟล์ JSON ([Pairing](docs/th/pairing.md))
- **เส้นทางเชื่อมต่อและ host key ที่ปักหมุดไว้.** แต่ละ Target มีเส้นทาง LAN, Tailscale (บริการเครือข่ายส่วนตัว) และ public ได้ ลองตามลำดับ และตั้ง ProxyJump (ผ่านเครื่อง "ตัวกลาง") เพิ่มได้ host key ถูกปักหมุดไว้ และถ้า host key ไม่ตรง ระบบจะหยุดสำรองเส้นทางทั้งหมด
- **หลาย Master.** หลาย Master ที่เป็นอิสระต่อกันควบคุม Target เครื่องเดียวกันได้ โดยแต่ละตัวมีกุญแจของตัวเอง Master หนึ่งสามารถ merge เพื่อสั่ง Target ของ Master อื่นผ่านเครื่องนั้นได้ หนึ่งทอด และทำสองทางได้ Master หนึ่งเครื่องเก็บ Target ได้สูงสุด 50 เครื่อง
- **ตรวจสถานะแบบเบา.** สำหรับเส้นทางตรง การตรวจประจำจะอ่านเฉพาะ banner ของ SSH และไม่ login
- **แอป desktop.** ไอคอน tray พร้อมหน้าต่างจัดการ (UI ภาษาไทย) มีผังและตารางของเครื่องทั้งหมด เพิ่มเครื่อง งานและ log ตัวแก้ไฟล์ และปุ่ม "เปิด Shell" โปรเซสของหน้าต่างเริ่มเมื่อเรียกใช้ และ service ทำงานต่อได้โดยไม่ต้องมีหน้าต่าง
- **รันด้วยสิทธิ์ของ Owner ที่ login อยู่.** Target ใหม่รันงานด้วยบัญชี Owner ที่ติดตั้งไว้ (Windows: token แบบผู้ดูแลระบบของ Owner; Ubuntu: UID/GID ของ Owner หรือ root เมื่อร้องขอ `asAdmin`) ส่วน Target ที่ยังอยู่ในโหมด service รุ่นก่อนจะรัน job และไฟล์เป็น SYSTEM หรือ root และตอบ `E_EXECUTION_MODE` เมื่อใช้ terminal และ `sshcc shell` ([Owner session](docs/OWNER-SESSION.md))
- **ไม่มี listener บนเครือข่ายของตัวเอง.** CLI, MCP server และ tray คุยกับ service ผ่าน named pipe ของ Windows หรือ Unix socket บนเครื่องเดียวกัน และมีการตรวจตัวตนผู้เรียก เครื่องมือนี้ไม่เปิดพอร์ตเครือข่าย แต่พอร์ต SSH ของแต่ละ Target ก็ยังต้องให้ Master เข้าถึงได้

เครื่อง Target ที่ตั้งเป้าไว้: Windows 10/11 x64, Windows Server 2019/2022/2025, Ubuntu 24.04 x64 การทดสอบยังไม่ครอบคลุมทุกรุ่น ([INSTALL.md](docs/INSTALL.md), [VALIDATION.md](docs/VALIDATION.md))

## เริ่มต้นอย่างรวดเร็ว

ตัวติดตั้งยังไม่ได้ลงนามดิจิทัล จึงให้ build จากซอร์ส และควรติดตั้งบน VM ทดลองก่อน เพราะตัวติดตั้งจะตั้ง service ที่มีสิทธิ์สูง

**เครื่องเดียวทำอะไรได้บ้าง.** ขั้นตอน build, ทดสอบ และ smoke test ด้านล่างทั้งหมดรันบนเครื่อง Windows หรือ Ubuntu เครื่องเดียวได้ ไม่ติดตั้ง service และไม่ติดต่อเครื่องอื่น ส่วนการสร้างแพ็กเกจสำหรับติดตั้งต่างออกไป ดูหัวข้อ "แพ็กเกจสำหรับติดตั้ง" ด้านล่าง เพราะแพ็กเกจ Master แบบเต็มต้องใช้เครื่อง build สองเครื่อง ถ้าอยากดู OpenSSH จริงระหว่าง Master สองเครื่องกับ Target ห้าเครื่องโดยไม่ต้องติดตั้งตัวผลิตภัณฑ์ ให้ใช้ชุดทดสอบ Docker ([tests/containers/README.md](tests/containers/README.md) และ [VALIDATION.md หัวข้อ 3](docs/VALIDATION.md#3-real-ssh-container-harness)) ซึ่งต้องมี Docker

**สิ่งที่ต้องมี.** Git สำหรับดึงโค้ด, Rust 1.90 ขึ้นไป (เวอร์ชันที่ประกาศไว้ใน `Cargo.toml`) ให้ติดตั้งด้วย [rustup](https://rustup.rs) แล้วตรวจด้วย `rustc --version` เพราะแพ็กเกจ `cargo` ใน repository เริ่มต้นของลินุกซ์ดิสโทรอาจเก่ากว่า 1.90, native C/C++ compiler (ระหว่าง build จะคอมไพล์ SQLite ที่แถมมาด้วย), OpenSSH client (`ssh` และ `ssh-keygen` ที่อยู่ใน PATH ซึ่ง `sshcc` เรียกใช้เพื่อเชื่อมต่อ Target และสร้างกุญแจของ Master และชุดทดสอบก็ใช้) และ Python 3 สำหรับ smoke test และสคริปต์ build (ต้องเป็น 3.11 ขึ้นไปสำหรับ `scripts/build-deb.sh` เพราะใช้ `tomllib`)

- **Ubuntu 24.04:** `sudo apt install git curl build-essential pkg-config libsystemd-dev openssh-client` เพราะ core ลิงก์กับ `libsystemd` และ `curl` ใช้สำหรับติดตั้ง rustup ถ้าจะ build แอป desktop ให้ติดตั้ง `libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev` เพิ่ม
- **Windows:** ต้องมี Git for Windows, C/C++ compiler ตามข้างบน (กับ toolchain ปกติของ Rust มักเป็นชุด C++ ของ Visual Studio Build Tools) และ OpenSSH Client ซึ่งเป็นฟีเจอร์เสริมของ Windows (ตรวจด้วย `ssh -V`) เครื่องมือบรรทัดคำสั่งไม่ต้องใช้ไลบรารีอื่นเพิ่ม แอป desktop ต้องมี WebView2 และ toolchain ของ Tauri 2 บน Windows ส่วนตัวติดตั้ง Windows ต้องมี NSIS `makensis` ([docs/BUILD-TOOLS.md](docs/BUILD-TOOLS.md))

**ดึงโค้ดมา.** รันทุกคำสั่งด้านล่างจากโฟลเดอร์นี้

```bash
git clone https://github.com/VK-butter/SSH-control-center
cd SSH-control-center
```

**Build ทดสอบ และลองใช้.** smoke test ใช้ state ชั่วคราวแบบส่วนตัว ไม่ติดตั้ง service และไม่ติดต่อเครื่องอื่น

```powershell
# Windows (PowerShell)
cargo test --workspace --exclude sshcc-desktop
cargo build --release -p sshcc
python scripts/smoke.py --sshcc target\release\sshcc.exe
```

```bash
# Ubuntu 24.04
cargo test --workspace --exclude sshcc-desktop
cargo build --release -p sshcc
python3 scripts/smoke.py --sshcc target/release/sshcc
```

การ build ครั้งแรกต้องคอมไพล์ dependency ทั้งหมด รวมถึง SQLite ที่แถมมาด้วย จึงใช้เวลานาน บน Windows ถ้าหาคำสั่ง `python` ไม่เจอ ให้ใช้ `py` แทน smoke test ที่ผ่านจะจบด้วยการพิมพ์รายงาน JSON ที่มี `"passed": true` และออกด้วย exit code 0 ถ้ามีการตรวจข้อใดไม่ผ่าน จะหยุดพร้อมข้อผิดพลาดและ exit code ที่ไม่ใช่ 0

**แอป desktop (ไม่บังคับ).** `cargo build --release -p sshcc-desktop`

**แพ็กเกจสำหรับติดตั้ง.** ไฟล์ setup EXE ของ Windows และไฟล์ `.deb` ของ Ubuntu สร้างผ่านหลายขั้นตอน (สร้าง thin template ก่อน แล้วค่อยสร้างตัวติดตั้ง Master แบบเต็ม) แพ็กเกจ Master แบบเต็มของระบบใดก็ตามต้องใช้ template ครบทั้งสองตัว จึงต้องมีเครื่อง Windows หนึ่งเครื่อง (พร้อม NSIS) และเครื่อง Ubuntu 24.04 x86_64 หนึ่งเครื่อง (เป็น VM ก็ได้) และต้องคัดลอกไฟล์ระหว่างสองเครื่อง `scripts/build-windows.ps1` จะหยุดการ build Master แบบเต็มถ้า template ขาดไปตัวใดตัวหนึ่ง และ `scripts/build-deb.sh --setup` จะหยุดถ้าไม่มี template ของ Windows ที่ตรึงไว้และ manifest ของมัน ถ้ามีเครื่องเดียว ให้หยุดที่ smoke test ด้านบน ทำตาม [Build packages](docs/INSTALL.md#build-packages) โดย `scripts/build-deb.sh` ใช้ได้เฉพาะบน Ubuntu 24.04 x86_64 จากนั้นเพิ่มเครื่องด้วย [Pairing](docs/th/pairing.md) (ขั้นตอนพร้อมหน้าจอภาษาไทย: [QUICKSTART.th.md](docs/QUICKSTART.th.md))

บน Target แบบ Ubuntu ที่ใช้โหมด Owner ตัว service ต้องมี cgroup v2 และ `Delegate=yes` ไฟล์ `installers/ubuntu/sshcc.service` ที่แถมมาตั้งค่านี้ไว้แล้ว

## เอกสาร

| หัวข้อ | English | ไทย |
|---|---|---|
| ทำงานอย่างไร (ภาษาง่าย ๆ) | [how-it-works](docs/how-it-works.md) | [how-it-works](docs/th/how-it-works.md) |
| สถาปัตยกรรม | [architecture](docs/architecture.md) | [architecture](docs/th/architecture.md), [ARCHITECTURE.th](docs/ARCHITECTURE.th.md) |
| การจับคู่เครื่อง | [pairing](docs/pairing.md) | [pairing](docs/th/pairing.md), [QUICKSTART.th](docs/QUICKSTART.th.md) |
| โมเดลความปลอดภัย | [security-model](docs/security-model.md) | [security-model](docs/th/security-model.md) |
| การรายงานช่องโหว่ | [SECURITY](SECURITY.md) | มีเฉพาะภาษาอังกฤษ |
| คำถามที่พบบ่อย | [faq](docs/faq.md) | [faq](docs/th/faq.md) |
| ติดตั้ง อัปเกรด ถอนการติดตั้ง | [INSTALL](docs/INSTALL.md) | มีเฉพาะภาษาอังกฤษ |
| การใช้ CLI และ MCP | [AGENT](docs/AGENT.md) | มีเฉพาะภาษาอังกฤษ |
| Owner session และ terminal | [OWNER-SESSION](docs/OWNER-SESSION.md) | มีเฉพาะภาษาอังกฤษ |
| สัญญา API | [CONTRACT](docs/CONTRACT.md) | มีเฉพาะภาษาอังกฤษ |
| เครื่องมือสำหรับ build | [BUILD-TOOLS](docs/BUILD-TOOLS.md) | มีเฉพาะภาษาอังกฤษ |
| การทดสอบและสิ่งที่ยังไม่ครอบคลุม | [VALIDATION](docs/VALIDATION.md) | มีเฉพาะภาษาอังกฤษ |
| ข้อจำกัดของเครื่องมือ MCP และบันทึกการปรับแต่ง (ทบทวนจากโค้ด ไม่ได้วัดผลจริง) | [ai-control-performance](docs/ai-control-performance.md) | มีเฉพาะภาษาอังกฤษ |
| ศัพท์ที่ใช้ในโค้ดและ ADR | [GLOSSARY](docs/GLOSSARY.md) | มีเฉพาะภาษาอังกฤษ |
| บันทึกการตัดสินใจด้านออกแบบ | [ADR 0001](docs/adr/0001-mcp-registration-no-overwrite.md) | มีเฉพาะภาษาอังกฤษ |
| แอป desktop บันทึกสำหรับนักพัฒนา | [desktop/README](desktop/README.md) | มีเฉพาะภาษาอังกฤษ |
| ชุดทดสอบ UI ของ desktop | [UI-FIXTURE](desktop/tests/UI-FIXTURE.md) | มีเฉพาะภาษาอังกฤษ |
| ชุดทดสอบ Docker ที่ใช้ OpenSSH จริง | [tests/containers/README](tests/containers/README.md) | มีเฉพาะภาษาอังกฤษ |
| ปุ่มเปิด Shell ในแอป | มีเฉพาะภาษาไทย | [SHELL-BUTTON](docs/SHELL-BUTTON.md) |

## สถานะและข้อจำกัดที่ทราบ

เวอร์ชัน 0.4.0 เป็นโปรเจกต์โอเพนซอร์สส่วนตัวของ VK-butter ยังไม่พร้อมใช้งานจริง ช่องว่างที่ทราบ:

- **แพลตฟอร์ม.** รองรับเฉพาะ Windows และ Ubuntu 24.04 x64 ไม่รองรับ macOS เพราะไม่มีตัวติดตั้ง และตัว executor บน Unix ใช้ฟีเจอร์ systemd และ cgroup ที่มีเฉพาะ Linux
- **ตัวติดตั้งยังไม่ลงนาม.** `SHA256SUMS` ตรวจความสมบูรณ์ของไฟล์ ไม่ได้บอกว่าใครเป็นผู้สร้างไฟล์
- **UI ของ desktop และหน้าจอตัวติดตั้งเป็นภาษาไทย** (มีศัพท์เทคนิคภาษาอังกฤษปน) ยังไม่มี UI ฉบับแปลเป็นอังกฤษ ส่วน CLI และเครื่องมือ MCP เป็นภาษาอังกฤษ
- **อำนาจกว้าง.** Master ทุกตัวที่ลงทะเบียนไว้มีสิทธิ์ระดับผู้ดูแลระบบบน Target ของตน `asAdmin` เป็นแค่ตัวเลือกต่อคำขอ ไม่ใช่ขั้นตอนขออนุมัติแยก และโปรเซสใดก็ตามที่รันด้วยบัญชี Owner ในเครื่องสามารถสั่งคำสั่งเหล่านี้ไปยังทุก Target ได้
- **กุญแจของ Master ไม่มี passphrase.** private SSH key ของ Master ถูกสร้างโดยไม่มี passphrase สิทธิ์ของไฟล์จึงเป็นการป้องกันเพียงอย่างเดียว ใครอ่านไฟล์นี้ได้ก็เข้าถึงทุก Target ของกลุ่มนั้นได้ จนกว่าคุณจะ revoke Master นั้น
- **Prompt injection.** เครื่องมือนี้รันสคริปต์ทุกอย่างที่ได้รับ AI agent ที่อ่านข้อความที่ไว้ใจไม่ได้ (log, ไฟล์, หน้าเว็บ) อาจถูกหลอกให้ส่งคำสั่งที่เป็นอันตราย ควรตรวจสอบสิ่งที่ agent ทำ
- **รหัสจับคู่.** checksum ของรหัสช่วยจับความเสียหายจากการคัดลอกและวาง ไม่ใช่ลายเซ็นดิจิทัล ควรส่งผ่านช่องทางที่ไว้ใจได้
- **Job เก็บสคริปต์และผลลัพธ์** ไว้บน Target และ Master เก็บสำเนาคำขอของแต่ละ job (สคริปต์, โฟลเดอร์ทำงาน, environment) ไว้ในฐานข้อมูล fleet ของตน อย่าใส่รหัสผ่านในสคริปต์หรือค่า environment
- **โหมด Owner ต้องมี** Owner ที่ login อยู่ (Windows: ต้องเป็น administrator) และบน Ubuntu ต้องมี cgroup v2 พร้อม `Delegate=yes`
- **ตัวช่วยเน้น Codex ก่อน.** การตั้งค่า Master จะเพิ่มรายการ `ssh_control_center` ลงในค่าตั้งของ Codex (พร้อมสำรองไฟล์) เมื่อตรวจพบ Codex ส่วน Cursor และ Claude Code (และ Codex ถ้าตอนตั้งค่าข้ามไป) เชื่อมจากแอป desktop
- **ช่องว่างด้านการทดสอบ.** ไม่มีชุดทดสอบใดรันตัวติดตั้ง Windows ตั้งแต่ต้นจนจบ และกรณี WAN จริง, Tailscale, การรีบูต และ logout ยังไม่ได้ทดสอบ ([VALIDATION.md](docs/VALIDATION.md#not-covered-by-anything-in-this-tree)) ซอร์สชุดนี้ไม่มีการตั้งค่า CI และไม่ได้อ้างว่าผ่านการตรวจสอบความปลอดภัยโดยบุคคลที่สาม

## ความปลอดภัย

private key อยู่บน Master และไม่ถูกส่งออก (กุญแจของ Master ไม่มี passphrase ดูข้อจำกัดด้านบน) host key ของ SSH ถูกปักหมุดไว้ และ forced command ของแต่ละ Target ผูกกับกุญแจของ Master ที่ลงทะเบียนไว้ อ่าน[โมเดลความปลอดภัย](docs/th/security-model.md)และ [SECURITY.md](SECURITY.md) ก่อนใช้กับเครื่องที่สำคัญ

## ข้อเสนอแนะ

ยินดีรับ Issues นี่เป็นโปรเจกต์ส่วนตัวที่ดูแลตามกำลังเท่าที่ทำได้ การตอบอาจช้า และไม่มีสัญญาว่าจะให้การสนับสนุน

## สัญญาอนุญาต

[MIT](LICENSE) ลิขสิทธิ์ (c) 2026 VK-butter

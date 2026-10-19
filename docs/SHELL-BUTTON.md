# เปิด Shell จาก SSH Control Center

1. เปิด SSH Control Center จากไอคอน tray
2. กด **เปิด Shell** ข้างชื่อเครื่อง หรือเปิดรายละเอียดเครื่องแล้วกดปุ่มเดียวกัน
3. หาก Owner มีหลาย session ระบบเลือก session ที่ active/ล่าสุดให้อัตโนมัติ หรือเลือกเองในกล่องโต้ตอบเมื่อต้องการล็อก session

โปรแกรมเปิดหน้าต่าง Terminal แยก โดยใช้บัญชีและสิทธิ์ตามการติดตั้งของเครื่องปลายทาง เริ่มใน home ของ Owner: Windows ใช้ PowerShell และ Ubuntu ใช้ login shell

ปิดหน้าจัดการแล้ว Shell ยังทำงานต่อได้ ใช้ `Ctrl+]` เพื่อ detach หรือพิมพ์ `exit` เพื่อจบ Shell

เครื่องปลายทางต้องมี Target service ที่รองรับ terminal โปรแกรมตรวจการเชื่อมต่อและ Owner ก่อนเปิด หากยังไม่พร้อมจะแสดงเหตุผลในหน้าจัดการ

CLI ที่เทียบเท่าปุ่มนี้คือ `sshcc shell <Target>`

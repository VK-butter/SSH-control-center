const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {spawn} = require('node:child_process');
const artifacts=path.resolve(__dirname,'../../artifacts/shell-button-ui');fs.mkdirSync(artifacts,{recursive:true});
const {chromium} = require('playwright');
(async()=>{
  const browser=await chromium.launch({headless:true,channel:'msedge'}); const results=[];
  try {
    for(const single of [false,true]) {
      const server=spawn(process.execPath,[path.join(__dirname,'fixture.cjs')],{env:{...process.env,SSHCC_UI_TEST_PORT:'0',SSHCC_FIXTURE_SINGLE_SESSION:single?'1':'0'}});
      try {
        const url=await new Promise((resolve,reject)=>{server.stdout.on('data',data=>{const m=String(data).match(/http:\/\/127\.0\.0\.1:\d+/);if(m)resolve(m[0]);});server.once('error',reject);});
        const page=await browser.newPage({viewport:{width:1280,height:900}});await page.goto(url);
        const button=page.locator('[data-action="open-shell"]');await button.waitFor();await button.click();
        if(!single) {
          await page.locator('#shell-dialog[open]').waitFor();
          assert.equal(await page.locator('#shell-session').inputValue(),'');
          await page.locator('[data-action="close-shell-dialog"]').click();
          assert.equal((await (await page.request.get(url+'/calls')).json()).filter(x=>x.command==='open_shell').length,1);
          await button.click();await page.locator('#shell-session').selectOption('7');
          await page.locator('#open-shell-session button[type="submit"]').click();
        }
        await page.locator('#notice').filter({hasText:'ในหน้าต่าง Terminal แล้ว'}).waitFor();
        const calls=(await (await page.request.get(url+'/calls')).json()).filter(x=>x.command==='open_shell');
        assert.equal(calls.at(-1).targetId,'ui-test');assert.equal(calls.at(-1).ownerSessionId,single?null:'7');
        assert.equal(await page.locator('#shell-dialog').evaluate(x=>x.open),false);
        await page.locator('[data-action="select-target"]').first().click();
        await page.locator('.page-heading [data-action="open-shell"]').click();
        if(!single) {await page.locator('#shell-dialog[open]').waitFor();await page.locator('#shell-session').selectOption('3');await page.locator('#open-shell-session button[type="submit"]').click();}
        await page.locator('#notice').filter({hasText:'ในหน้าต่าง Terminal แล้ว'}).waitFor();
        const all=(await (await page.request.get(url+'/calls')).json()).filter(x=>x.command==='open_shell');
        assert.equal(all.at(-1).targetId,'ui-test');assert.equal(all.at(-1).ownerSessionId,single?null:'3');
        await page.screenshot({path:path.join(artifacts,'shell-button-ui-'+(single?'single':'multiple')+'.png')});
        results.push({singleSession:single,rowButton:true,detailsButton:true,correctTargetId:true,explicitSessionOrDefault:true,cancelDoesNotLaunch:true});
        await page.close();
      } finally {server.kill();}
    }
    fs.writeFileSync(path.join(artifacts,'shell-button-ui-results.json'),JSON.stringify({ok:true,results},null,2)+'\n');console.log(JSON.stringify({ok:true,scenarios:results.length}));
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});

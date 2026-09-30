import test from "node:test";
import assert from "node:assert/strict";
import {create,setup} from "../src/host_facilities/store.mjs";
import {database,scope} from "./d1-fixture.mjs";
const event=(id,source="management/default",scopeId="deploy")=>({id,event_name:"management-operation",source_instance:source,action:"completed",outcome:"success",severity:"info",actor_kind:"user",actor_id:"alice",actor_display:null,scope_module:null,scope_type:"deployment",scope_id:scopeId,scope_display:null,resource_type:"operation",resource_id:"op1",resource_display:null,correlation_id:"op1",causation_id:null,request_id:"op1",story_id:null,reason:null,metadata:{intent_digest:"a".repeat(64)},occurred_at:"2026-09-30T01:02:03Z"});
const input=id=>({event:event(id),occurred_key:"2026-09-30T01:02:03.000000000Z"});
test("explicit setup, primary session, persisted caller source and stable append",async()=>{const db=database();let store=create(db,scope(),{profile:"workers-d1"});await assert.rejects(store.readiness());assert.equal(db.calls.batches,0);await setup(db);await store.readiness();const first=await store.append(input("event1"));const replay=await store.append(input("event1"));assert.deepEqual(replay,first);assert.equal(first.source_instance,"management/default");store=create(db,scope(),{profile:"workers-d1"});assert.deepEqual(await store.get("event1"),first);assert.deepEqual(db.calls.sessions,Array(6).fill("first-primary"));db.close();});
test("lost append reply leaves one persisted event, bounded cursor/scope query and event fence",async()=>{const db=database();await setup(db);const s=scope(),store=create(db,s,{profile:"workers-d1"});db.calls.failAfterCommit=true;await assert.rejects(store.append(input("e2")),/lost_reply/);assert.equal(db.calls.batches,2);assert.equal((await store.get("e2")).id,"e2");await store.append(input("e1"));const rows=await store.list({scope_type:"deployment",scope_id:"deploy",limit:1});assert.deepEqual(rows.map(r=>r.id),["e2","e1"]);assert.deepEqual((await store.list({scope_type:"deployment",scope_id:"other",limit:1})),[]);assert.deepEqual((await store.list({limit:1,cursor:{occurred_at:"2026-09-30T01:02:03.000000000Z",id:"e2"}})).map(r=>r.id),["e1"]);s.closed=true;await assert.rejects(store.get("e1"),/event_closed/);db.close();});

test("wrong profile and an incompatible ledger fail before migration",async()=>{const db=database();assert.throws(()=>create(db,scope(),{profile:"native-pg"}));await setup(db);const session=db.withSession("first-primary");await session.batch([session.prepare("UPDATE lenso_audit_schema SET version=99 WHERE singleton=1")]);const before=db.calls.batches;await assert.rejects(setup(db),/incompatible/);assert.equal(db.calls.batches,before);db.close();});

test("malformed append receipt stays unknown after commit and read recovers the single event",async()=>{const db=database();await setup(db);const store=create(db,scope(),{profile:"workers-d1"});db.calls.malformedAfterCommit=true;await assert.rejects(store.append(input("receipt-lost")),/receipt_unknown/);assert.equal((await store.get("receipt-lost")).id,"receipt-lost");assert.equal(db.calls.batches,2);db.close();});

test("setup refuses an existing unowned aggregate instead of silently adopting it",async()=>{const db=database(),session=db.withSession("first-primary");await session.batch([session.prepare("CREATE TABLE audit_events(foreign_id TEXT)")]);const before=db.calls.batches;await assert.rejects(setup(db),/schema_unmanaged/);assert.equal(db.calls.batches,before);db.close();});

test("a fresh bounded read after readiness observes a concurrent persisted event",async()=>{
 let current=null,sessions=0;
 const db={withSession(mode){assert.equal(mode,"first-primary");sessions++;const snapshot=current;return {prepare(sql){return {bind(){return this;},async first(){return sql.includes("lenso_audit_schema")?{version:1}:snapshot;}};}};}};
 const store=create(db,scope(),{profile:"workers-d1"});
 await store.readiness();assert.equal(await store.get("fresh"),null);
 current={event_json:JSON.stringify(event("fresh"))};
 assert.equal((await store.get("fresh")).id,"fresh");assert.equal(sessions,3);
});

// Exact native command discovery only; no execute, UI activation or download.
#include <Core/UserInterface/CommandDefinitions.h>
#include <Core/UserInterface/CommandDefinition.h>
static_assert(sizeof(AfMcmasterState)==12,"McMaster state ABI");
extern "C" int af_mcmaster_discover(AfContext*c,AfDocumentRef*d,AfMcmasterState*out,AfBytes sink,void*user,AfExtrudeDiagnostic*diag) noexcept {
    if(!out||!sink||!diag)return 1;*out={};*diag={};diag->sdk_error=-1;diag->stage=1;
    try{
        Ptr<adsk::fusion::Design> design;int status=af_design(c,d,design);if(status)return status;
        auto intent=design->designIntent();if(!ex_ok(c,diag,300))return 7;out->design_intent=static_cast<uint32_t>(intent);
        auto ui=c->app->userInterface();if(!ex_ok(c,diag,301)||!ui)return 7;
        auto definitions=ui->commandDefinitions();if(!ex_ok(c,diag,302)||!definitions)return 7;
        auto command=definitions->itemById("InsertMcMasterCarrComponentCommand");if(!ex_ok(c,diag,303))return 7;
        if(!command)return 0;
        bool valid=command->isValid();if(!ex_ok(c,diag,304)||!valid)return 3;
        out->available=1;
        auto id=command->id();if(!ex_ok(c,diag,305))return 7;
        auto name=command->name();if(!ex_ok(c,diag,306))return 7;
        out->native=command->isNative()?1:0;if(!ex_ok(c,diag,307))return 7;
        if(!sink(user,0,id.data(),id.size())||!sink(user,1,name.data(),name.size()))return 5;
        return 0;
    }catch(...){return 4;}
}

"""Configuration graph, stale-target and recovery boundaries without a host."""
import csv
import importlib
import math
from pathlib import Path
import tempfile
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch

from test_mechanical import adsk, c, m, Entity, Collection, MeshCollection, component

config = importlib.import_module('mechanical_test_commands.mechanical.configurations')
bom = importlib.import_module('mechanical_test_commands.mechanical.bill_of_materials')
tx = importlib.import_module('mechanical_test_commands.mechanical.transactions')


class Configurations(unittest.TestCase):
    def setUp(self):
        c._entities.clear(); c._documents.clear(); config._sources.clear()
        values = NS(NumericParameterValueType=0, TextParameterValueType=1)
        self.enums = patch.object(adsk.fusion, 'ParameterValueTypes', values, create=True)
        self.enums.start(); self.addCleanup(self.enums.stop)
        self.casts = set()

    def native(self, kind, **kwargs):
        namespace = adsk.core if kind in ('PropertyGroup', 'StringProperty') else adsk.fusion
        if (namespace.__name__, kind) not in self.casts:
            change = patch.object(namespace, kind, NS(cast=lambda entity: entity), create=True)
            change.start(); self.addCleanup(change.stop); self.casts.add((namespace.__name__, kind))
        return Entity(objectType='adsk::' + ('core' if namespace is adsk.core else 'fusion') + '::' + kind, **kwargs)

    def fixture(self, kind='ConfigurationParameterColumn', text=False):
        self.root = self.native('Component', name='Bracket', partNumber='PN-001', description='fixture', entityToken='root',
            revisionId='same', meshBodies=MeshCollection(), bRepBodies=Collection(), sketches=Collection(), features=Collection(),
            occurrences=Collection(), allOccurrences=Collection(), material=None)
        self.timeline = Collection(); self.timeline.markerPosition = 0
        self.design = NS(isValid=True, isConfiguredDesign=True,isConfiguration=False,configurationRowId="", rootComponent=self.root, allComponents=Collection([self.root]),
            allParameters=Collection(), timeline=self.timeline,
            unitsManager=NS(isValidExpression=lambda expression, unit: expression.strip() == '1 ' + unit,
                            convert=lambda value, source, unit: value * 10 if unit == 'mm' else value))
        self.root.parentDesign = self.design
        self.tokens = {'root': [self.root]}
        self.design.findEntityByToken = lambda token: self.tokens.get(token, [])
        self.table = self.native('ConfigurationTopTable', id='T', name='Sizes', hasCustomColumns=False,
                                testRow=None, customThemeTables=Collection())
        for field in ('appearanceTable', 'materialTable', 'plasticRuleTable', 'sheetMetalRuleTable'):
            setattr(self.table, field, self.native('ConfigurationThemeTable', id=field, rows=Collection(), columns=Collection()))
        self.rows = [self.native('ConfigurationRow', id='R'+str(index), name=name, index=index,
                                parentTable=self.table, parentRow=None, childRows=[], isTestRow=False)
                     for index, name in enumerate(('Base', 'Wide'))]
        self.table.rows = Collection(self.rows); self.table.activeRow = self.rows[0]
        self.column = self.native(kind, id='C', index=0, title='Width', rowCount=2, parentTable=self.table)
        self.cells = [self.native(kind.replace('Column', 'Cell'), parentRow=row, parentColumn=self.column)
                      for row in self.rows]
        if kind == 'ConfigurationParameterColumn':
            parameter = self.native('UserParameter', name='Width', unit='mm', expression='20 mm', entityToken='width', design=self.design)
            self.design.allParameters = Collection([parameter]); self.tokens['width'] = [parameter]
            self.column.parameter = parameter
            for index, cell in enumerate(self.cells):
                cell.expression = "'blue'" if text else str(20 + 10 * index) + ' mm'
                cell.valueType = 1 if text else 0
                if text: cell.textValue = 'blue'
                else: cell.value = 2 + index
        elif kind in ('ConfigurationSuppressColumn', 'ConfigurationVisibilityColumn'):
            source = self.native('Occurrence', component=self.root, entityToken='occ', assemblyContext=None,
                                 nativeObject=None, fullPathName='Part:1', isConfiguration=False)
            self.tokens['occ'] = [source]
            setattr(self.column, 'feature' if kind == 'ConfigurationSuppressColumn' else 'entity', source)
            for cell in self.cells:
                cell.isSuppressed = False; cell.isEffectivelySuppressed = True; cell.isVisible = False
        elif kind == 'ConfigurationPropertyColumn':
            group = self.native('PropertyGroup', id='component-properties', parent=self.root)
            self.column.parentProperty = self.native('StringProperty', id='part-number', name='Localized title',
                                                     parent=group, isReadOnly=False)
            for index, cell in enumerate(self.cells): cell.value = 'PN-' + str(index)
        self.column.getCellByRowId = lambda row_id: self.cells[next(i for i, r in enumerate(self.rows) if r.id == row_id)]
        self.table.columns = Collection([self.column]); self.design.configurationTopTable = self.table
        self.doc = Entity(name='Untitled', isSaved=False, isModified=True); self.design.parentDocument = self.doc
        self.app = NS(activeDocument=self.doc, activeProduct=self.design)
        return self.design

    def test_unconfigured_preserves_normal_read_and_never_requests_table(self):
        design = NS(isConfiguredDesign=False,isConfiguration=False,configurationRowId="", rootComponent=NS(allOccurrences=Collection()))
        result = config.snapshot(design, strict=True)
        self.assertFalse(result['configured']); self.assertTrue(result['complete']); self.assertIsNone(result['table'])
        self.assertFalse(result['configurationWritesEnabled'])

    def test_generated_configuration_identity_is_bound_and_all_strict_paths_refuse(self):
        self.fixture(); self.design.isConfiguredDesign = False
        self.design.isConfiguration = True; self.design.configurationRowId = 'Generated-A'
        graph = config.snapshot(self.design)
        self.assertFalse(graph['complete']); self.assertFalse(graph['configured'])
        self.assertTrue(graph['isConfiguration']); self.assertEqual(graph['configurationRowId'], 'Generated-A')
        self.assertEqual(config.provenance(graph)['rowId'], 'Generated-A')
        first = c.revision(self.design); self.design.configurationRowId = 'Generated-B'
        self.assertNotEqual(first, c.revision(self.design))
        value = c.current_target(self.app)
        for action in (lambda:config.snapshot(self.design, strict=True),
                       lambda:c.target(self.app, {'target':value}, True),
                       lambda:c.target(self.app, {'target':value}, True, model_write=False),
                       lambda:c.recovery_snapshot(self.design),
                       lambda:bom.generate(self.design, value, {})):
            with self.assertRaises(c.MechanicalError) as failure: action()
            self.assertEqual(failure.exception.code, 'configuration_scope_unsupported')

    def test_missing_or_contradictory_single_configuration_identity_cannot_be_complete(self):
        for missing in ('isConfiguration', 'configurationRowId'):
            self.fixture(); delattr(self.design, missing)
            self.assertFalse(config.snapshot(self.design)['complete'])
            with self.assertRaises(c.MechanicalError):config.snapshot(self.design, strict=True)
        for flag, row_id in ((False, 'unexpected'), (True, ''), (False, None)):
            self.fixture(); self.design.isConfiguration = flag; self.design.configurationRowId = row_id
            self.assertFalse(config.snapshot(self.design)['complete'])

    def test_numeric_and_text_cells_use_separate_native_getters_and_units(self):
        self.fixture(); data = config.snapshot(self.design, strict=True)['table']['cells'][0]
        self.assertEqual((data['value'], data['unit'], data['internalValue']), (20, 'mm', 2))
        self.fixture(text=True); data = config.snapshot(self.design, strict=True)['table']['cells'][0]
        self.assertEqual(data['textValue'], 'blue'); self.assertNotIn('internalValue', data)

    def test_conversion_error_sentinel_cannot_become_a_valid_value(self):
        self.fixture(); self.design.unitsManager.convert = lambda *args: -1
        result = config.snapshot(self.design)
        self.assertFalse(result['complete'])
        self.assertIn('basis failed', result['readErrors'][0]['message'])
        with self.assertRaises(c.MechanicalError): config.snapshot(self.design, strict=True)

    def test_signed_values_including_minus_one_survive_verified_linear_units(self):
        for unit, factor in (('mm', 10), ('deg', 180 / math.pi), ('kg', 1), ('mm2', 100), ('mm3', 1000), ('', 1)):
            with self.subTest(unit=unit):
                self.fixture(); self.column.parameter.unit = unit
                self.design.unitsManager.isValidExpression = lambda expression, target: expression.strip() == ('1 ' + target).strip()
                calls = []
                self.design.unitsManager.convert = lambda value, source, target: calls.append((value, source, target)) or factor
                self.cells[0].value = -1 / factor
                self.cells[1].value = -2.5 / factor
                result = config.snapshot(self.design, strict=True)['table']['cells']
                self.assertAlmostEqual(result[0]['value'], -1)
                self.assertAlmostEqual(result[1]['value'], -2.5)
                self.assertTrue(all(call[0] == 1 for call in calls))

    def test_unsupported_units_refuse_instead_of_accepting_fallback_conversion(self):
        self.fixture(); self.column.parameter.unit = 'K'
        with patch.object(self.design.unitsManager, 'convert') as conversion:
            result = config.snapshot(self.design)
        self.assertFalse(result['complete']); conversion.assert_not_called()
        self.assertEqual(result['readErrors'][0]['code'], 'configuration_scope_unsupported')

    def test_bare_number_in_default_seconds_is_not_proof_of_unitless(self):
        self.fixture(); self.column.parameter.unit = 's'
        self.design.unitsManager.isValidExpression = lambda expression, unit: expression.strip() == '1'
        with patch.object(self.design.unitsManager, 'convert', return_value=1) as conversion:
            result = config.snapshot(self.design)
        self.assertFalse(result['complete']); conversion.assert_not_called()
        self.assertEqual(result['readErrors'][0]['code'], 'configuration_scope_unsupported')

    def test_inactive_cell_change_invalidates_revision_and_recovery_witness(self):
        self.fixture(); before = c.revision(self.design); witness = c.recovery_snapshot(self.design)
        value = c.current_target(self.app)
        self.cells[1].expression = '40 mm'; self.cells[1].value = 4
        self.assertNotEqual(before, c.revision(self.design))
        self.assertFalse(tx.equivalent(witness, c.recovery_snapshot(self.design)))
        with self.assertRaises(c.MechanicalError) as failure: c.target(self.app, {'target': value}, True, model_write=False)
        self.assertEqual(failure.exception.code, 'stale_revision')
        self.cells[1].expression = '30 mm'; self.cells[1].value = 3
        self.assertTrue(tx.equivalent(witness, c.recovery_snapshot(self.design)))

    def test_active_row_and_graph_links_are_witnessed_even_with_equal_values(self):
        self.fixture(); before = config.snapshot(self.design, strict=True)
        self.table.activeRow = self.rows[1]
        self.assertNotEqual(before, config.snapshot(self.design, strict=True))
        self.table.activeRow = self.rows[0]
        replacement = self.native('UserParameter', name='Width', unit='mm', expression='20 mm', entityToken='replacement', design=self.design)
        self.design.allParameters.values.append(replacement); self.tokens['replacement'] = [replacement]
        self.column.parameter = replacement
        self.assertNotEqual(before, config.snapshot(self.design, strict=True))

    def test_invalidated_source_wrapper_rebinds_by_fresh_api_equality(self):
        self.fixture(); before = config.snapshot(self.design, strict=True)
        old = self.column.parameter
        fresh = self.native('UserParameter', name='Width', unit='mm', expression='20 mm', entityToken='fresh-token', design=self.design)
        old.isValid = False; self.tokens['width'] = [fresh]; self.tokens['fresh-token'] = [fresh]
        self.column.parameter = fresh; self.design.allParameters = Collection([fresh])
        self.assertEqual(before, config.snapshot(self.design, strict=True))
        distinct = self.native('UserParameter', name='Width', unit='mm', entityToken='other')
        self.tokens['width'] = [fresh, distinct]
        with self.assertRaises(c.MechanicalError): config.snapshot(self.design, strict=True)

    def test_same_property_id_on_distinct_owner_does_not_merge(self):
        self.fixture('ConfigurationPropertyColumn'); before = config.snapshot(self.design, strict=True)
        other = self.native('Component', name='Bracket', entityToken='other')
        self.tokens['other'] = [other]; self.design.allComponents.values.append(other)
        self.column.parentProperty.parent.parent = other
        self.assertNotEqual(before, config.snapshot(self.design, strict=True))

    def test_child_model_parameter_owner_is_native_component_and_user_parameter_is_design_scoped(self):
        self.fixture()
        user = config.snapshot(self.design, strict=True)['table']['columns'][0]['source']
        self.assertEqual((user['ownerId'], user['ownerScope']), (c.identity(self.design), 'design'))
        child = self.native('Component', name='Child', entityToken='child')
        self.design.allComponents.values.append(child)
        model = self.native('ModelParameter', name='d1', unit='mm', expression='20 mm', entityToken='model', component=child)
        self.design.allParameters.values.append(model); self.tokens['model'] = [model]; self.column.parameter = model
        source = config.snapshot(self.design, strict=True)['table']['columns'][0]['source']
        self.assertEqual((source['ownerId'], source['ownerScope']), (c.identity(child), 'component'))
        self.assertNotEqual(source['ownerId'], c.identity(self.root))
        model.component = self.native('Component', name='Child', entityToken='foreign')
        self.assertFalse(config.snapshot(self.design)['complete'])

    def test_missing_preview_getters_keep_stable_cells_but_never_authorize_writes(self):
        self.fixture(); del self.table.testRow; del self.rows[0].parentRow; del self.rows[1].childRows; del self.rows[1].isTestRow
        result = config.snapshot(self.design)
        self.assertFalse(result['complete'])
        self.assertEqual(len(result['table']['rows']), 2)
        self.assertEqual(len(result['table']['columns']), 1)
        self.assertEqual([cell['value'] for cell in result['table']['cells']], [20, 30])
        self.assertEqual({error['field'] for error in result['readErrors']},
                         {'top.testRow', 'row.R0.parentRow', 'row.R1.childRows', 'row.R1.isTestRow'})
        with self.assertRaises(c.MechanicalError): config.snapshot(self.design, strict=True)

    def add_reserved_row(self):
        row = self.native('ConfigurationRow', id='native-test-row', name='Test', index=0,
                          parentTable=self.table, parentRow=None, childRows=[], isTestRow=True)
        self.table.testRow = row
        for index, ordinary in enumerate(self.rows): ordinary.index = index + 1
        cell = self.native(self.column.objectType.split('::')[-1].replace('Column','Cell'),
                           parentRow=row, parentColumn=self.column)
        if self.column.objectType.endswith('ParameterColumn'):
            cell.expression='-10 mm';cell.valueType=0;cell.value=-1
        else: cell.value='TEST-PART'
        previous=self.column.getCellByRowId
        self.column.getCellByRowId=lambda row_id:cell if row_id==row.id else previous(row_id)
        self.column.rowCount=len(self.rows)+1
        return row,cell

    def test_reserved_row_is_separate_and_complete_cell_graph_drives_stale_guard(self):
        self.fixture();row,cell=self.add_reserved_row()
        graph=config.snapshot(self.design,strict=True)
        self.assertEqual(len(graph['table']['rows']),2)
        self.assertEqual(graph['table']['testRow']['id'],row.id)
        self.assertEqual(graph['table']['testRow']['scope'],'test')
        self.assertEqual([(entry['rowId'],entry['scope']) for entry in graph['table']['cells']],
                         [('R0','ordinary'),('R1','ordinary'),(row.id,'test')])
        self.assertEqual(graph['table']['cells'][-1]['value'],-10)
        value=c.current_target(self.app);before=c.recovery_snapshot(self.design)
        with self.assertRaises(c.MechanicalError) as refusal:c.target(self.app,{'target':value},True)
        self.assertEqual(refusal.exception.code,'configuration_mutation_unverified')
        cell.expression='-20 mm';cell.value=-2
        self.assertFalse(tx.equivalent(before,c.recovery_snapshot(self.design)))
        with self.assertRaises(c.MechanicalError) as stale:c.target(self.app,{'target':value},True,model_write=False)
        self.assertEqual(stale.exception.code,'stale_revision')

    def test_offline_property_cells_and_unreadable_source_preserve_remaining_cells(self):
        self.fixture('ConfigurationPropertyColumn');row,reserved=self.add_reserved_row()
        class OfflineCell:
            def __init__(self, original):self.__dict__.update(original.__dict__)
            @property
            def value(self):raise RuntimeError('Properties are unavailable in offline mode or unsaved component')
        bad=OfflineCell(self.cells[0]); original=self.column.getCellByRowId
        self.column.getCellByRowId=lambda row_id:bad if row_id=='R0' else original(row_id)
        self.column.parentProperty.parent.parent=NS(objectType='adsk::fusion::Component',isValid=False)
        graph=config.snapshot(self.design)
        self.assertFalse(graph['complete']);self.assertEqual(len(graph['table']['cells']),3)
        self.assertEqual(graph['table']['cells'][0]['value'],{'unavailable':True})
        self.assertEqual(graph['table']['cells'][1]['value'],'PN-1')
        self.assertEqual(graph['table']['cells'][2]['value'],'TEST-PART')
        self.assertEqual({error['field'] for error in graph['readErrors']},{'column.C.property','cell.R0.C.value'})
        with self.assertRaises(c.MechanicalError):config.snapshot(self.design,strict=True)

    def test_reserved_row_overlap_flags_hierarchy_and_count_mismatch_remain_incomplete(self):
        for field,value in (('isTestRow',False),('parentRow','parent'),('childRows','children')):
            self.fixture();row,_=self.add_reserved_row()
            setattr(row,field,self.rows[0] if value=='parent' else self.rows if value=='children' else value)
            self.assertFalse(config.snapshot(self.design)['complete'])
        self.fixture();self.table.testRow=self.rows[0]
        self.assertFalse(config.snapshot(self.design)['complete'])
        self.fixture();self.add_reserved_row();self.column.rowCount=4
        graph=config.snapshot(self.design);self.assertFalse(graph['complete'])
        self.assertEqual(len(graph['table']['cells']),3)
        self.assertEqual(graph['readErrors'][0]['field'],'column.C.rowCount')

    def test_missing_reserved_cell_address_retains_all_other_cells_and_blocks_witness(self):
        self.fixture();row,_=self.add_reserved_row();reader=self.column.getCellByRowId
        self.column.getCellByRowId=lambda row_id:None if row_id==row.id else reader(row_id)
        graph=config.snapshot(self.design);self.assertFalse(graph['complete'])
        self.assertEqual([cell['value'] for cell in graph['table']['cells'][:2]],[20,30])
        self.assertTrue(graph['table']['cells'][-1]['unavailable'])
        with self.assertRaises(c.MechanicalError):config.snapshot(self.design,strict=True)

    def test_incomplete_scopes_refuse_before_transaction_or_compute(self):
        edits = [lambda: setattr(self.table, 'hasCustomColumns', True),
                 lambda: setattr(self.rows[1], 'parentRow', self.rows[0]),
                 lambda: self.table.customThemeTables.values.append(self.table.appearanceTable),
                 lambda: setattr(self.column, 'objectType', 'adsk::fusion::ConfigurationFeatureAspectColumn'),
                 lambda: self.column.__dict__.pop('parameter')]
        for edit in edits:
            with self.subTest(edit=edit):
                self.fixture(); value = c.current_target(self.app); edit()
                result = config.snapshot(self.design)
                self.assertFalse(result['complete'])
                with patch.object(tx, '_verified', True), patch.object(self.app, 'executeTextCommand', create=True) as command:
                    with self.assertRaises(c.MechanicalError) as failure:
                        tx.execute(self.app, {'target': value}, lambda *_: self.fail('mutation executed'))
                    self.assertEqual(failure.exception.code, 'configuration_scope_unsupported'); command.assert_not_called()
                with self.assertRaises(c.MechanicalError): c.recovery_snapshot(self.design)

    def test_missing_configured_flag_is_unknown_and_refuses_strict_witness(self):
        result = config.snapshot(NS())
        self.assertIsNone(result['configured']); self.assertFalse(result['complete'])
        with self.assertRaises(c.MechanicalError): config.snapshot(NS(), strict=True)

    def test_complete_configured_design_still_refuses_indirect_model_writes_before_native_gate(self):
        self.fixture(); value = c.current_target(self.app)
        with self.assertRaises(c.MechanicalError) as failure: c.target(self.app, {'target': value}, True)
        self.assertEqual(failure.exception.code, 'configuration_mutation_unverified')
        self.assertEqual(c.target(self.app, {'target': value}, True, model_write=False)[2], value)

    def test_external_configuration_blocks_standard_design_mutations(self):
        self.fixture(); self.design.isConfiguredDesign = False
        occurrence = self.native('Occurrence', fullPathName='External:1', isConfiguration=True,
            configurationRow=self.rows[1], configuredDataFile=Entity(id='source-file', versionNumber=7))
        self.root.allOccurrences = Collection([occurrence])
        result = config.snapshot(self.design)
        self.assertFalse(result['complete']); self.assertFalse(result['configured'])
        self.assertEqual(result['configuredOccurrences'][0]['sourceVersion'], 7)
        with self.assertRaises(c.MechanicalError): config.snapshot(self.design, strict=True)

    def test_bad_cell_address_and_duplicate_ids_and_budget_never_claim_complete(self):
        for edit in (lambda: setattr(self.cells[0], 'parentRow', self.rows[1]),
                     lambda: setattr(self.rows[1], 'id', self.rows[0].id),
                     lambda: setattr(self.cells[0], 'value', float('nan'))):
            self.fixture(); edit(); self.assertFalse(config.snapshot(self.design)['complete'])
        self.fixture()
        with patch.object(config, 'MAX_CELLS', 1):
            with patch.object(self.column, 'getCellByRowId') as reader:
                self.assertFalse(config.snapshot(self.design)['complete']); reader.assert_not_called()

    def test_explicit_effective_suppression_and_visibility_remain_distinct(self):
        self.fixture('ConfigurationSuppressColumn')
        cell = config.snapshot(self.design, strict=True)['table']['cells'][0]
        self.assertFalse(cell['isSuppressed']); self.assertTrue(cell['isEffectivelySuppressed'])
        self.fixture('ConfigurationVisibilityColumn')
        self.root.bRepBodies = component('body', 1).bRepBodies
        value = {'documentId': 'D', 'revision': 'R'}
        with patch.object(bom, 'ref', return_value={}): result = bom.generate(self.design, value, {})
        self.assertEqual(result['totalPurchasingInstances'], 1)
        self.assertEqual(result['configuration']['rowId'], 'R0')

    def test_both_csv_modes_stamp_target_and_active_configuration_without_activation(self):
        self.fixture('ConfigurationPropertyColumn'); self.table.activeRow = self.rows[1]
        self.root.bRepBodies = component('body', 1).bRepBodies
        value = {'documentId': 'D', 'revision': 'revision'}
        with tempfile.TemporaryDirectory() as directory, patch.object(bom, 'target', return_value=(self.doc, self.design, value)), patch.object(bom, 'ref', return_value={}):
            for mode in ('flat', 'structured'):
                path = Path(directory) / (mode + '.csv')
                result = bom.bom_export(None, {'filename': str(path), 'mode': mode})
                with path.open() as stream: row = next(csv.DictReader(stream))
                self.assertEqual([row[key] for key in ('documentId', 'revision', 'configurationTableId', 'configurationRowId', 'configurationRowName')],
                                 ['D', 'revision', 'T', 'R1', 'Wide'])
                self.assertEqual(result['configuration']['rowId'], 'R1')
                self.assertIs(self.table.activeRow, self.rows[1])

    def test_discovery_exposes_only_read_operation_and_rejects_configuration_writes(self):
        self.assertIn('configuration_inspect', m.OPERATIONS)
        self.assertNotIn('configuration_inspect', m.WRITES)
        self.assertEqual([name for name in m.SCHEMA if name.startswith('configuration_')], ['configuration_inspect'])
        for op in ('configuration_enable', 'configuration_activate', 'configuration_cells_update'):
            self.assertEqual(m.handle_mechanical(None, {'op': op})['errorCode'], 'unsupported_operation')


if __name__ == '__main__': unittest.main()

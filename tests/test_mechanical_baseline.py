"""Writes on a design that already carries faults, offline: the faults present before a write are its
baseline; a write may not add one, change one or leave one on what it edited. A sketch relationship the
graph reader cannot read refuses only a write that edits or reads that sketch."""
import importlib
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch

from test_mechanical import Collection, Entity, adsk, c, ops

compute = importlib.import_module(c.__package__ + '.compute')
tx = importlib.import_module(c.__package__ + '.transactions')
rb = importlib.import_module(c.__package__ + '.readback')


def fault(name, message='lost reference', state=2, index='4'):
    return {'index': index, 'name': name, 'state': state, 'message': message}


def health(*faults, unknown=(), computed=None):
    return {'healthy': computed is not False and not faults and not unknown, 'computeSucceeded': computed,
            'complete': not unknown, 'faults': list(faults), 'unknown': list(unknown), 'suppressed': [], 'timelineCount': 9}


OLD = [fault('Fillet7', 'edge lost', index='2'), fault('Plane3', 'face lost', index='5')]


class Judge(unittest.TestCase):
    def test_a_write_that_adds_nothing_is_allowed_on_a_faulted_model(self):
        compute.judge(health(*OLD), health(*OLD))

    def test_a_new_fault_refuses_and_is_named(self):
        with self.assertRaises(c.MechanicalError) as error:
            compute.judge(health(*OLD, fault('Extrude9', 'no profile', index='8')), health(*OLD))
        self.assertEqual(error.exception.code, 'unhealthy_model')
        self.assertEqual([f['name'] for f in error.exception.details['newFaults']], ['Extrude9'])
        self.assertEqual(error.exception.details['preExistingFaults']['count'], 2)

    def test_a_changed_fault_refuses(self):
        worse = [OLD[0], fault('Plane3', 'face lost', state=1, index='5')]
        with self.assertRaises(c.MechanicalError) as error:
            compute.judge(health(*worse), health(*OLD))
        self.assertEqual(error.exception.details['newFaults'], [worse[1]])
        other = [OLD[0], fault('Plane3', 'another message', index='5')]
        with self.assertRaises(c.MechanicalError):
            compute.judge(health(*other), health(*OLD))

    def test_an_earlier_delete_moves_indexes_without_reading_as_a_change(self):
        moved = [dict(row, index=str(int(row['index']) - 1)) for row in OLD]
        compute.judge(health(*moved), health(*OLD))

    def test_lookalike_faults_count(self):
        two = [fault('Boss1', index='3'), fault('Boss1', index='6')]
        compute.judge(health(*two), health(*two))
        with self.assertRaises(c.MechanicalError) as error:
            compute.judge(health(*two, fault('Boss1', index='9')), health(*two))
        self.assertEqual([f['index'] for f in error.exception.details['newFaults']], ['9'])

    def test_a_cleared_fault_is_allowed_and_reported(self):
        compute.judge(health(OLD[1]), health(*OLD))
        self.assertEqual(compute.summary(health(*OLD), health(OLD[1]))['cleared'], {'count': 1, 'faults': [OLD[0]]})

    def test_unreadable_items_are_faults_too(self):
        lost = {'index': '7', 'name': 'Pattern2', 'error': 'InternalValidationError'}
        compute.judge(health(unknown=[lost]), health(unknown=[lost]))
        with self.assertRaises(c.MechanicalError):
            compute.judge(health(unknown=[lost]), health())

    def test_a_healthy_baseline_keeps_the_old_rule(self):
        with self.assertRaises(c.MechanicalError):
            compute.judge(health(fault('Fillet1')), health())
        with self.assertRaises(c.MechanicalError) as error:
            compute.judge(health(computed=False), health())
        self.assertEqual(str(error.exception), 'Recompute failed.')
        # a failed full recompute refuses on a faulted model too: Fusion's computeAll succeeds there
        with self.assertRaises(c.MechanicalError):
            compute.judge(health(*OLD, computed=False), health(*OLD))

    def test_a_fault_left_on_what_the_write_edits_is_its_own(self):
        feature = Entity(objectType='adsk::fusion::FilletFeature', name='Fillet7')
        natives = [(OLD[0], NS(entity=feature)), (OLD[1], NS(entity=Entity(objectType='adsk::fusion::ConstructionPlane')))]
        compute.judge(health(*OLD), health(*OLD), natives, [Entity(objectType='adsk::fusion::BRepFace')])
        with self.assertRaises(c.MechanicalError) as error:
            compute.judge(health(*OLD), health(*OLD), natives, [feature])
        self.assertEqual(error.exception.details['editedFaults'], [OLD[0]])
        self.assertEqual(str(error.exception), 'The write left a fault on an entity it edited.')

    def test_the_subjects_are_what_a_write_edits_in_place_not_its_inputs(self):
        feature = Entity(objectType='adsk::fusion::FilletFeature', name='Fillet7', entityToken='tok-fillet')
        axis = Entity(objectType='adsk::fusion::ConstructionAxis', name='Axis1', entityToken='tok-axis')
        tokens = {'tok-fillet': feature, 'tok-axis': axis}
        owned = Entity(name='d5', createdBy=feature)
        design = NS(findEntityByToken=lambda token: [tokens[token]] if token in tokens else [],
                    allParameters=Collection([owned, Entity(name='width')]))
        value = {'documentId': 'd', 'revision': 'r'}
        ref = lambda token, kind: dict(value, token=token, objectType='adsk::fusion::' + kind)
        self.assertEqual(compute.subjects(design, value, {'op': 'feature_edit', 'feature': ref('tok-fillet', 'FilletFeature')}), [feature])
        self.assertEqual(compute.subjects(design, value, {'op': 'feature_create', 'axis': ref('tok-axis', 'ConstructionAxis')}), [])
        self.assertEqual(compute.subjects(design, value, {'op': 'feature_edit', 'feature': ref('tok-gone', 'FilletFeature')}), [])
        batch = {'op': 'parameter_batch', 'changes': [{'action': 'set', 'name': 'd5'}, {'action': 'set', 'name': 'width'},
                                                     {'action': 'create', 'name': 'x'}]}
        self.assertEqual(compute.subjects(design, value, batch), [feature])

    def test_a_reply_lists_the_first_few_and_counts_the_rest(self):
        many = [fault('F%d' % i, index=str(i)) for i in range(15)]
        out = compute.summary(health(*many), health(*many))
        self.assertEqual((out['count'], len(out['faults']), out['more']), (15, 5, 10))
        self.assertNotIn('cleared', out)
        self.assertEqual(compute.summary(health()), {'count': 0, 'faults': []})


class Walk(unittest.TestCase):
    def test_the_walk_hands_back_the_items_behind_its_faults(self):
        class Broken:
            name, isGroup, isSuppressed = 'Pattern2', False, False

            @property
            def healthState(self):
                raise RuntimeError('InternalValidationError')
        bad = NS(name='Fillet7', isGroup=False, isSuppressed=False, healthState=2, errorOrWarningMessage='edge lost')
        good = NS(name='Boss1', isGroup=False, isSuppressed=False, healthState=0)
        design = NS(timeline=Collection([good, bad, Broken()]))
        natives = []
        state = c.health(design, False, natives)
        self.assertEqual(state['faults'], [{'index': '1', 'name': 'Fillet7', 'state': 2, 'message': 'edge lost'}])
        self.assertEqual(state['unknown'], [{'index': '2', 'error': 'InternalValidationError', 'name': 'Pattern2'}])
        self.assertEqual([item for _, item in natives][:1], [bad])
        self.assertEqual(len(natives), 2)

    def test_a_check_inside_a_write_is_held_to_its_baseline(self):
        with patch.object(compute, 'health', lambda design, recompute=False, natives=None: health(*OLD)):
            with c.write_context(health(*OLD)):
                compute.midway(None)
            with c.write_context(health(OLD[0])), self.assertRaises(c.MechanicalError):
                compute.midway(None)
            with self.assertRaises(c.MechanicalError):
                compute.midway(None)  # outside a write the baseline is a healthy model


class Transactions(unittest.TestCase):
    """The adapter on a faulted model: the faults are the baseline, a breaking write is undone and the
    undo is verified against the same baseline."""
    def setUp(self):
        compute.reset()
        self.state = [health(*OLD)]
        self.reads = []
        self.commands = []
        self.patches = [patch.object(compute, 'health', self.read),
                        patch.object(tx, 'health', self.read),
                        patch.object(tx, '_verified', True),
                        patch.object(tx, 'target', lambda app, args, write=False: ('doc', self.design, {'documentId': 'd', 'revision': 'r0'})),
                        patch.object(tx, 'current_target', lambda app: {'documentId': 'd', 'revision': 'r1'}),
                        patch.object(tx, 'recovery_snapshot', lambda design, health_state=None, strict_sketches=(): {'faults': len(self.state[0]['faults'])}),
                        patch.object(tx, 'same', lambda a, b: True),
                        patch.object(tx, 'command', self.command)]
        self.design = NS(timeline=NS(markerPosition=3, count=3), findEntityByToken=lambda token: [])
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()

    def read(self, design, recompute=False, natives=None):
        self.reads.append(recompute)
        return dict(self.state[0], computeSucceeded=True if recompute else None)

    def command(self, app, text):
        self.commands.append(text.split()[0])
        if text.startswith('PTransaction.Abort'):
            self.state[0] = health(*OLD)  # the undo puts the model back

    def test_a_write_elsewhere_commits_and_reports_the_faults_already_there(self):
        out = tx.execute(NS(activeDocument='doc'), {'op': 'feature_create', 'target': {}}, lambda design, value, args: {'made': 1})
        self.assertEqual(self.commands, ['PTransaction.Start', 'PTransaction.Commit'])
        self.assertEqual((out['preExistingFaults']['count'], out['health']['healthy']), (2, False))
        # the next write's baseline is what this one verified, faults included, without another walk
        self.assertEqual(compute.health_before(None, 'd', 'r1')['faults'], OLD)

    def test_a_write_that_breaks_something_is_undone_and_the_undo_verified(self):
        def breaks(design, value, args):
            self.state[0] = health(*OLD, fault('Extrude9', 'no profile', index='8'))
            return {'made': 1}
        with self.assertRaises(c.MechanicalError) as error:
            tx.execute(NS(activeDocument='doc'), {'op': 'feature_create', 'target': {}}, breaks)
        details = error.exception.details
        self.assertEqual((error.exception.code, details['causeCode']), ('mutation_failed', 'unhealthy_model'))
        self.assertEqual([f['name'] for f in details['causeDetails']['newFaults']], ['Extrude9'])
        self.assertEqual(self.commands, ['PTransaction.Start', 'PTransaction.Abort'])
        self.assertTrue(details['recovery']['verified'], details['recovery'])

    def test_the_probe_passes_on_a_faulted_model(self):
        params = {}
        design = NS(userParameters=NS(add=lambda name, value, unit, comment: params.setdefault(name, NS(value=0.2, deleteMe=lambda: params.pop(name) is not None)),
                                      itemByName=lambda name: params.get(name)))

        def command(app, text):
            self.commands.append(text.split()[0])
            if text.startswith('PTransaction.Abort'):
                params.clear()
        with patch.object(tx, 'target', lambda app, args, write=False: ('doc', design, {'documentId': 'd', 'revision': 'r0'})), \
                patch.object(tx, 'command', command), patch.object(tx, '_verified', False), \
                patch.object(adsk.core, 'ValueInput', NS(createByString=lambda text: text), create=True):
            out = tx._probe(NS(version='test'), {'target': {}})
            self.assertTrue(out['transaction']['verified'])
            self.assertEqual(out['preExistingFaults']['count'], 2)


class Sketch(unittest.TestCase):
    """A sketch whose graph holds a relationship the reader does not support."""
    def sketch(self):
        point = lambda x, y: Entity(objectType='adsk::fusion::SketchPoint', geometry=NS(x=x, y=y, z=0), isFixed=False)
        a, b = point(0, 0), point(1, 0)
        line = Entity(objectType='adsk::fusion::SketchLine', isConstruction=False, isFixed=False,
                      geometry=NS(objectType='adsk::core::Line3D', startPoint=a.geometry, endPoint=b.geometry),
                      startSketchPoint=a, endSketchPoint=b)
        horizontal = Entity(objectType='adsk::fusion::HorizontalConstraint', line=line)
        polygon = Entity(objectType='adsk::fusion::PolygonConstraint')
        dimension = Entity(objectType='adsk::fusion::SketchLinearDimension', entityOne=a, entityTwo=b, isDriving=True,
                           value=1.0, orientation=0, parameter=NS(name='d1', expression='10 mm'))
        sketch = Entity(objectType='adsk::fusion::Sketch', name='Hex', isFullyConstrained=False,
                        sketchCurves=Collection([line]), sketchPoints=Collection([a, b]),
                        geometricConstraints=Collection([horizontal]), sketchDimensions=Collection([dimension]))
        return sketch, polygon

    def test_a_readable_sketch_reads_the_same_either_way(self):
        sketch, _ = self.sketch()
        self.assertEqual(rb.sketch_data(sketch), rb.sketch_data(sketch, strict=False))
        self.assertNotIn('constraintGraph', rb.sketch_data(sketch, strict=False))

    def test_strict_refuses_and_lenient_lists_the_relationship(self):
        sketch, polygon = self.sketch()
        sketch.geometricConstraints.values.append(polygon)
        with self.assertRaises(c.MechanicalError) as error:
            rb.sketch_data(sketch)
        self.assertEqual(error.exception.code, 'unsupported_constraint_graph')
        data = rb.sketch_data(sketch, strict=False)
        self.assertEqual(data['constraints'][1]['type'], 'adsk::fusion::PolygonConstraint')
        self.assertEqual(data['constraintGraph']['complete'], False)
        self.assertEqual([row['type'] for row in data['constraintGraph']['unreadable']], ['adsk::fusion::PolygonConstraint'])
        # the readable part of the graph is still there
        self.assertEqual(data['constraints'][0]['links'], {'line': {'collection': 'curves', 'index': 0}})
        self.assertEqual(data['dimensions'][0]['name'], 'd1')

    def test_only_a_sketch_the_write_names_must_read_completely(self):
        sketch, polygon = self.sketch()
        sketch.geometricConstraints.values.append(polygon)
        other, _ = self.sketch()
        component = Entity(objectType='adsk::fusion::Component', name='root', sketches=Collection([sketch, other]),
                           bRepBodies=Collection(), features=Collection())
        design = NS(rootComponent=NS(allOccurrences=Collection()), allComponents=Collection([component]),
                    allParameters=Collection(), timeline=NS(count=0, markerPosition=0))
        with patch.object(c, 'components_by_occurrence', lambda design, occurrences, size: [component]), \
                patch('mechanical_test_commands.mechanical.configurations.snapshot', lambda design, strict=False: {}), \
                patch('mechanical_test_commands.mechanical.assembly_state.component_state', lambda comp: {}):
            snapshot = c.recovery_snapshot(design, health())
            self.assertEqual(snapshot['components'][0][3][0]['constraintGraph']['complete'], False)
            c.recovery_snapshot(design, health(), [other])
            with self.assertRaises(c.MechanicalError) as error:
                c.recovery_snapshot(design, health(), [sketch])
            self.assertEqual(error.exception.code, 'unsupported_constraint_graph')

    def test_reading_a_sketch_lists_what_it_cannot_read_instead_of_refusing(self):
        sketch = Entity(objectType='adsk::fusion::Sketch', name='Hex')
        with patch.object(ops, 'target', return_value=(None, None, {'documentId': 'd', 'revision': 'r'})), \
                patch.object(ops, 'resolve', return_value=sketch), \
                patch.object(ops, 'ref', return_value={'token': 'tok-hex'}), \
                patch.object(rb, 'sketch_data', lambda entity, value=None, strict=True: {'strict': strict}):
            out = ops.inspect_entity(None, {'entity': {}})
        self.assertEqual(out['entity']['strict'], False)

    def test_the_sketches_a_write_names(self):
        sketch, _ = self.sketch()
        line = sketch.sketchCurves.item(0)
        line.parentSketch = sketch
        found = {'tok-sketch': sketch, 'tok-line': line}
        design = NS(findEntityByToken=lambda token: [found[token]] if token in found else [])
        value = {'documentId': 'd', 'revision': 'r'}
        ref = lambda token, kind: dict(value, token=token, objectType='adsk::fusion::' + kind)
        self.assertEqual(tx.sketches_named(design, value, {'sketch': ref('tok-sketch', 'Sketch')}), [sketch])
        self.assertEqual(tx.sketches_named(design, value, {'entities': [ref('tok-line', 'SketchLine')]}), [sketch])
        # a profile's sketch is read only for the profile; other references and the target are skipped
        profile = dict(value, objectType='adsk::fusion::Profile', sketchToken='tok-sketch', profileIndex=0)
        face = ref('tok-face', 'BRepFace')
        self.assertEqual(tx.sketches_named(design, value, {'target': ref('tok-sketch', 'Sketch'), 'profiles': [profile], 'faces': [face]}), [])
        # a reference that does not resolve is left for the operation
        self.assertEqual(tx.sketches_named(design, value, {'sketch': ref('tok-gone', 'Sketch')}), [])


class Parameters(unittest.TestCase):
    def setUp(self):
        compute.reset()
        self.fillet = Entity(objectType='adsk::fusion::FilletFeature', name='Fillet7')

    def run_set(self, owner, faults_after):
        p = Entity(name='d3', expression='10 mm', unit='mm', createdBy=owner)
        design = NS(allParameters=Collection([p]), unitsManager=NS(isValidExpression=lambda *a: True))
        reads = []

        def read(design, recompute=False, natives=None):
            reads.append(recompute)
            rows = faults_after if p.expression == '12 mm' else OLD
            if natives is not None:
                natives.extend((row, NS(entity=self.fillet if row['name'] == 'Fillet7' else None)) for row in rows)
            return health(*rows, computed=True if recompute else None)
        with patch.object(ops, 'target', return_value=(None, design, {'documentId': 'd', 'revision': 'r'})), \
                patch.object(compute, 'health', read), \
                patch.object(ops, 'recovery_snapshot', side_effect=lambda d, health_state=None: {'expression': p.expression}), \
                patch.object(ops, 'current_target', return_value={'documentId': 'd', 'revision': 'r2'}), \
                patch.object(ops, 'parameter_data', lambda design, parameter: {'name': parameter.name}):
            return ops.parameter_set(None, {'op': 'parameter_set', 'name': 'd3', 'expression': '12 mm'}), p

    def test_a_parameter_change_that_adds_nothing_succeeds(self):
        out, p = self.run_set(Entity(objectType='adsk::fusion::ExtrudeFeature', name='Extrude2'), OLD)
        self.assertEqual((p.expression, out['preExistingFaults']['count']), ('12 mm', 2))

    def test_a_parameter_of_a_faulted_feature_that_stays_faulted_is_refused(self):
        with self.assertRaises(c.MechanicalError) as error:
            self.run_set(self.fillet, OLD)
        self.assertEqual(error.exception.code, 'parameter_edit_failed')
        self.assertEqual(error.exception.details['causeDetails']['editedFaults'], [OLD[0]])
        self.assertTrue(error.exception.details['recovery']['verified'])


class Export(unittest.TestCase):
    def test_a_faulted_design_exports_and_reports_its_faults(self):
        compute.reset()
        with patch.object(ops, 'target', lambda app, args, write=False, model_write=True: ('doc', None, {'documentId': 'd', 'revision': 'r'})), \
                patch.object(ops, 'current_target', lambda app: {'documentId': 'd', 'revision': 'r'}), \
                patch.object(compute, 'health', lambda design, recompute=False, natives=None: health(*OLD)), \
                patch.object(ops, 'export_design', lambda design, args: {'path': 'out.step'}):
            out = ops.export(None, {'target': {}})
        self.assertEqual((out['path'], out['preExistingFaults']['count']), ('out.step', 2))


if __name__ == '__main__':
    unittest.main()

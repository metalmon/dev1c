# Тесты модуля scenario. Чистая логика разбора TOML — без gi/GTK.
import pathlib
import unittest

from placement import QUESTION_CHARS_PER_ROW
from scenario import TARGET_KINDS, load

_HERE = pathlib.Path(__file__).parent
SCENARIO = str(_HERE.parent / 'scenario.toml')
BROKEN_SCENARIO = str(_HERE / 'fixtures' / 'scenario_broken.toml')
MISSING_FIELD_SCENARIO = str(_HERE / 'fixtures' / 'scenario_missing_field.toml')
MISSING_ACTION_SCENARIO = str(_HERE / 'fixtures' / 'scenario_missing_action.toml')
BAD_ANCHOR_SCENARIO = str(_HERE / 'fixtures' / 'scenario_bad_anchor.toml')


class LoadTests(unittest.TestCase):
    def test_two_acts_numbered_through(self):
        steps = load(SCENARIO)
        self.assertEqual([s.number for s in steps], list(range(1, 16)))
        # Нумерация сквозная, а принадлежность к акту — отдельное поле:
        # человек видит «Шаг 1 из 7» в обоих актах (Machine.position).
        self.assertEqual([s.number for s in steps if s.act == 1], [1, 2, 3, 4, 5, 6, 7])
        self.assertEqual([s.number for s in steps if s.act == 2],
                         [8, 9, 10, 11, 12, 13, 14, 15])

    def test_texts_are_verbatim(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[1].title, 'Сначала знакомое')
        self.assertIn('заведите одну свою номенклатуру', steps[1].body)

    def _text(self, steps):
        # Все текстовые поля шага: заголовок, тело, кнопки и готовые
        # вопросы — это тоже текст, который видит человек.
        return ' '.join(' '.join((s.title, s.body, *s.buttons, *s.questions))
                        for s in steps).lower()

    def test_first_act_says_nothing_about_git(self):
        # В первом акте git и правда нет: человек открывает 1С, спрашивает
        # агента, смотрит ответ. Называть там инструмент незачем.
        steps = [s for s in load(SCENARIO) if s.act == 1]
        self.assertNotIn('git', self._text(steps))

    def test_second_act_names_git_and_teaches_the_command(self):
        # Правило изменено владельцем продукта 20.10.2026: если говорим
        # про git, называем его git и учим им пользоваться. Эвфемизм на
        # месте настоящей команды — обман: повторить у себя человек не
        # сможет.
        steps = [s for s in load(SCENARIO) if s.act == 2]
        text = self._text(steps)
        self.assertIn('git', text)
        self.assertIn('git clone', text)

    def test_unknown_target_kind_is_rejected(self):
        with self.assertRaises(ValueError):
            load(BROKEN_SCENARIO)   # цель kind = "луна"

    def test_bad_anchor_is_rejected_at_load_time(self):
        # Финальная волна: опечатка в anchor должна падать здесь, на
        # load(), а не когда-нибудь потом внутри targets.anchor_in на
        # живом стенде.
        with self.assertRaises(ValueError):
            load(BAD_ANCHOR_SCENARIO)   # anchor = "rihgt-bottom"

    def test_missing_required_field_is_rejected(self):
        with self.assertRaises(ValueError):
            load(MISSING_FIELD_SCENARIO)

    def test_target_kinds_constant(self):
        self.assertEqual(TARGET_KINDS,
                         ('none', 'desktop_icon', 'window', 'tray', 'connector'))

    def test_step_targets_match_brief(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[0].target.kind, 'none')
        self.assertEqual(steps[1].target.kind, 'desktop_icon')
        self.assertEqual(steps[1].target.desktop_file, '1c-start.desktop')
        self.assertEqual(steps[2].target.kind, 'desktop_icon')
        self.assertEqual(steps[2].target.desktop_file, 'dev.zed.Zed.desktop')
        self.assertEqual(steps[3].target.kind, 'window')
        self.assertEqual(steps[3].target.anchor, 'right-bottom')
        self.assertEqual(steps[4].target.kind, 'window')
        self.assertEqual(steps[4].target.anchor, 'right-bottom')
        self.assertEqual(steps[5].target.kind, 'tray')
        self.assertEqual(steps[6].target.kind, 'desktop_icon')
        self.assertEqual(steps[6].target.desktop_file, 'gitea.desktop')

    def test_no_step_carries_a_wait_for(self):
        # Шаги 2 и 3 (ruling второй и этой волны) wait_for не несут
        # вовсе: появление окна 1С/Zed значит «человек начал», не
        # «закончил» — вводить позицию (шаг 2) или нажимать кнопку,
        # которая закрывает 1С (шаг 3, пока Zed не открылся сам и шаг не
        # проскочил мимо неё). Остальные шаги никогда wait_for не несли.
        # Machine в его отсутствие падает обратно на target (см.
        # tests.test_steps) — поле у Step осталось, просто сценарию
        # больше негде им пользоваться.
        steps = load(SCENARIO)
        self.assertTrue(all(s.wait_for is None for s in steps))

    def test_advance_on(self):
        steps = load(SCENARIO)
        # Каждый шаг ждёт кнопку, без исключений — проверяем именно это,
        # а не длину списка: число шагов сценария меняется, правило нет.
        self.assertEqual({s.advance_on for s in steps}, {'button'})

    def test_last_step_has_only_done_button(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[-1].buttons, ('Готово',))

    def test_step_four_questions_are_separate_from_body(self):
        steps = load(SCENARIO)
        self.assertNotIn('Какая номенклатура', steps[3].body)
        self.assertEqual(steps[3].questions, (
            'Какая номенклатура есть в базе',
            'Покажи структуру документа «Заказ покупателя»',
            'Сколько справочников и документов в этой конфигурации',
        ))

    def test_ready_made_texts_fit_their_row(self):
        # Готовый текст рисуется ОДНОЙ строкой, без переноса
        # (callout._draw_questions, в отличие от тела шага), поэтому
        # лишнее не уедет вниз, а обрежется маской формы — тот самый
        # дефект, который на теле первого шага нашли только глазами на
        # стенде. Предел считает placement, здесь применяется к текстам.
        for s in load(SCENARIO):
            for text in s.questions:
                self.assertLessEqual(
                    len(text), QUESTION_CHARS_PER_ROW,
                    'шаг %d: готовый текст длиннее строки, его обрежет '
                    'форма выноски: %r' % (s.number, text))

    def test_the_edit_asked_of_the_agent_names_the_property(self):
        # Шаг 11 просит сменить синоним, а рядом в файле лежит имя
        # объекта с тем же словом «Номенклатура»; его правка ломает
        # загрузку конфигурации. Поэтому в готовом тексте названо само
        # свойство — текст написан под слабую модель, и это требование,
        # а не стиль.
        step = next(s for s in load(SCENARIO) if s.number == 11)
        self.assertEqual(len(step.questions), 1)
        self.assertIn('Synonym', step.questions[0])

    def test_the_circle_is_verified_by_asking_the_agent(self):
        # Шаг 14 проверяет круг вопросом агенту, а не списком в 1С:
        # подпись в списке берётся из синонима только при пустых
        # «Представлении объекта» и «Представлении списка», и на УНФ это
        # не проверено. Обещать то, чего не мерили, этот онбординг не
        # должен — а ответ агента из базы мерен дважды.
        step = next(s for s in load(SCENARIO) if s.number == 14)
        self.assertEqual(step.target.kind, 'window')
        self.assertEqual(len(step.questions), 1)
        self.assertNotIn('в списке', step.body)


class ActionTests(unittest.TestCase):
    # Подпись кнопки -> каноническое действие Machine.press ('next'/'close')
    # и, у двух кнопок, команда, которую нужно запустить. Задача 6: main.py
    # не хранит эту карту в коде, она лежит здесь же, рядом с подписями.
    def test_every_button_has_an_action(self):
        steps = load(SCENARIO)
        for s in steps:
            self.assertEqual(set(s.actions), set(s.buttons))

    def test_close_only_where_the_person_is_offered_a_way_out(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[0].actions['Закрыть'].action, 'close')
        self.assertEqual(steps[0].actions['Начать'].action, 'next')
        # Крестик/Esc закрывают любой шаг (это их дело, не сценария).
        # Среди кнопок сценария 'close' стоит ровно там, где человеку
        # предлагают выход: первый экран, конец первого акта («Готово»
        # вместо второго акта), развилка второго акта и его конец.
        closing = {s.number for s in steps
                   if any(a.action == 'close' for a in s.actions.values())}
        self.assertEqual(closing, {1, 7, 8, 15})

    def test_step_three_button_advances_and_closes_1c(self):
        # Кнопка сама двигает шаг («next» — wait_for у шага 3 больше нет,
        # ждать нечего) и вежливо просит 1С закрыться (close_window).
        steps = load(SCENARIO)
        action = steps[2].actions['Закрыть 1С и открыть редактор']
        self.assertEqual(action.action, 'next')
        self.assertEqual(action.close_window, 'Управление нашей фирмой')
        self.assertIsNone(steps[2].actions['Дальше'].close_window)

    def test_buttons_that_launch_something(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[1].actions['Открыть 1С'].launch, ('1c-stand', 'start1c'))
        self.assertIsNone(steps[1].actions['Дальше'].launch)
        self.assertEqual(steps[2].actions['Закрыть 1С и открыть редактор'].launch, ('zed',))
        self.assertIsNone(steps[2].actions['Дальше'].launch)

    def test_missing_action_is_rejected(self):
        with self.assertRaises(ValueError):
            load(MISSING_ACTION_SCENARIO)

    def test_actions_are_not_a_plain_mutable_dict(self):
        # Финальная волна: Step.actions по умолчанию был {} — один
        # разделяемый объект на все шаги без своих actions. Попытка
        # записи в него должна падать, а не тихо портить чужой default.
        steps = load(SCENARIO)
        with self.assertRaises(TypeError):
            steps[0].actions['новая'] = None


class ConnectorReserveTests(unittest.TestCase):
    """Шаг про значок в трее обязан беречь место окну коннектора.

    В трее стенда живёт только коннектор, и щелчок по значку открывает
    его окно в правом нижнем углу. Шаг, который зовёт туда щёлкнуть, а
    место не бережёт, ставит выноску ровно на будущее окно: человек
    нажимает и получает своё же окно под подсказкой.

    Поймано дважды — во втором акте 20.10.2026 и в первом 21.10.2026,
    второй раз потому, что правило применялось руками по одному шагу.
    Теперь его применяет тест.
    """

    def test_every_tray_step_reserves_the_connector_corner(self):
        steps = load(SCENARIO)
        tray = [s for s in steps if s.target.kind == 'tray']
        self.assertTrue(tray, 'шагов с целью в трее нет вовсе — правило стало бессмысленным')
        for s in tray:
            self.assertEqual(
                s.reserve, 'connector',
                'шаг %d показывает на значок в трее, но не бережёт место окну' % s.number)


if __name__ == '__main__':
    unittest.main()
